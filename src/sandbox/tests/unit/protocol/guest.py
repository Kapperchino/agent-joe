import asyncio
import errno
import os
import sys
import types
import unittest
from unittest.mock import AsyncMock, MagicMock, call, patch

guest = types.ModuleType("joe_guest")
sys.modules[guest.__name__] = guest
exec(compile(sys.stdin.read(), "joe-session.py", "exec"), guest.__dict__)


class WorkspaceTests(unittest.TestCase):
    def test_selected_worktree_is_bound_before_hiding_project(self):
        with (patch.object(guest.os, "open", side_effect=[10, 11, 12]) as opened,
              patch.object(guest.os, "close") as closed,
              patch.object(guest, "mount") as mounted,
              patch.object(guest.os, "chdir") as changed):
            guest.WorkspaceMount("/joe-project/.joe-worktrees/session").apply()
            flags = os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW
            self.assertEqual(opened.call_args_list, [
                call("/joe-project", flags),
                call(".joe-worktrees", flags, dir_fd=10),
                call("session", flags, dir_fd=11),
            ])
            self.assertEqual(closed.call_args_list, [call(12), call(11), call(10)])
            self.assertEqual([entry.args[1] for entry in mounted.call_args_list],
                             ["/workspace", "/joe-project", "/tmp", "/dev/shm"])
            self.assertEqual(mounted.call_args_list[0],
                             call("/proc/self/fd/12", "/workspace", None,
                                  guest.MountFlag.BIND | guest.MountFlag.RECURSIVE))
            self.assertTrue(mounted.call_args_list[1].args[3] & guest.MountFlag.READ_ONLY)
            changed.assert_called_once_with("/workspace")

    def test_traversal_is_rejected_before_opening_directories(self):
        with patch.object(guest.os, "open") as opened:
            for path in ["/workspace", "/joe-project/../other", "joe-project/a", "/joe-project-other"]:
                with self.assertRaises(ValueError):
                    guest.WorkspaceMount(path).apply()
            opened.assert_not_called()

    def test_symlink_selection_fails_without_mounting(self):
        with (patch.object(guest.os, "open", side_effect=[10, OSError(errno.ELOOP, "symlink")]),
              patch.object(guest.os, "close") as closed,
              patch.object(guest, "mount") as mounted):
            with self.assertRaises(OSError):
                guest.WorkspaceMount("/joe-project/link").apply()
            closed.assert_called_once_with(10)
            mounted.assert_not_called()

    def test_cache_server_stops_before_command_exit(self):
        with (patch.object(guest.subprocess, "run", return_value=types.SimpleNamespace(returncode=7)) as run,
              patch.object(guest.pathlib.Path, "is_socket", return_value=True)):
            with self.assertRaises(SystemExit) as result:
                guest.run_command(["/bin/false"])
            self.assertEqual(result.exception.code, 7)
            self.assertEqual(run.call_args_list[0].args, (["/bin/false"],))
            self.assertEqual(run.call_args_list[1].args, (["/usr/local/bin/sccache", "--stop-server"],))


class JobTests(unittest.IsolatedAsyncioTestCase):
    async def test_commands_get_private_namespaces_and_no_project_cwd(self):
        process = MagicMock()
        process.stdin.drain = AsyncMock()
        process.stdout.read = AsyncMock(return_value=b"")
        process.stderr.read = AsyncMock(return_value=b"")
        process.wait = AsyncMock()
        process.returncode = 0
        with (patch.object(guest, "refresh_metadata"),
              patch.object(guest, "emit") as emitted,
              patch.object(guest.asyncio, "create_subprocess_exec", AsyncMock(return_value=process)) as spawned):
            await guest.Job("identifier", {}, {}).run()
            arguments = spawned.call_args.args
            for option in ["--mount", "--pid", "--ipc", "--net", "--kill-child=KILL", "--mount-proc"]:
                self.assertIn(option, arguments)
            self.assertEqual(spawned.call_args.kwargs["cwd"], "/")
            self.assertEqual(spawned.call_args.kwargs["env"], {})
            self.assertEqual(emitted.call_args.args[0]["message"]["event"], "exited")


unittest.main()


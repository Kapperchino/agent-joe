import asyncio
import base64
import ctypes
import json
import os
import pathlib
import signal
import stat
import subprocess
import sys
import tty
from contextlib import ExitStack, suppress
from dataclasses import dataclass
from enum import Enum, IntFlag, auto


class Phase(Enum):
    STARTING = auto()
    RUNNING = auto()
    STOPPING = auto()
    COMPLETE = auto()


def emit(event):
    sys.stdout.write("joe-session:" + json.dumps(event, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def refresh_metadata():
    os.sync()
    with open("/proc/sys/vm/drop_caches", "w") as cache:
        cache.write("2\n")


class Access(Enum):
    READ_ONLY = auto()
    HIDDEN = auto()


class MountFlag(IntFlag):
    READ_ONLY = 1
    NO_SUID = 2
    NO_DEVICES = 4
    REMOUNT = 32
    BIND = 4096
    RECURSIVE = 16384


def mount(source, target, filesystem, flags, data=None):
    library = ctypes.CDLL(None, use_errno=True)
    operation = library.mount
    operation.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p,
                          ctypes.c_ulong, ctypes.c_char_p]
    operation.restype = ctypes.c_int
    encode = lambda value: os.fsencode(value) if value is not None else None
    result = operation(encode(source), encode(target), encode(filesystem), flags, encode(data))
    if result != 0:
        raise OSError(ctypes.get_errno(), "Cannot protect workspace mount", target)


@dataclass(frozen=True)
class ProtectedMount:
    path: str
    access: Access

    def open(self, descriptors):
        parts = pathlib.PurePosixPath(self.path).parts
        if parts[:2] != ("/", "workspace") or ".." in parts:
            raise ValueError("Protected paths must remain in the workspace")
        descriptor = os.open("/workspace", os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW)
        descriptors.callback(os.close, descriptor)
        for name in parts[2:]:
            descriptor = os.open(name, os.O_PATH | os.O_NOFOLLOW, dir_fd=descriptor)
            descriptors.callback(os.close, descriptor)
            if stat.S_ISLNK(os.fstat(descriptor).st_mode):
                raise ValueError("Protected paths cannot be symlinks")
        return descriptor

    def apply(self):
        read_only = MountFlag.READ_ONLY | MountFlag.NO_SUID | MountFlag.NO_DEVICES
        with ExitStack() as descriptors:
            descriptor = self.open(descriptors)
            target = f"/proc/self/fd/{descriptor}"
            match self.access:
                case Access.HIDDEN if stat.S_ISDIR(os.fstat(descriptor).st_mode):
                    mount("tmpfs", target, "tmpfs", read_only, "size=4096,mode=000")
                case Access.HIDDEN | Access.READ_ONLY:
                    source = "/dev/null" if self.access is Access.HIDDEN else target
                    recursive = MountFlag.RECURSIVE if self.access is Access.READ_ONLY else 0
                    mount(source, target, None, MountFlag.BIND | recursive)
                    mounted = self.open(descriptors)
                    mount(None, f"/proc/self/fd/{mounted}", None,
                          MountFlag.BIND | MountFlag.REMOUNT | read_only)


@dataclass(frozen=True)
class WorkspaceMount:
    path: str

    def apply(self):
        parts = pathlib.PurePosixPath(self.path).parts
        if parts[:2] != ("/", "joe-project") or ".." in parts:
            raise ValueError("Command workspaces must remain in the project")
        with ExitStack() as descriptors:
            flags = os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW
            descriptor = os.open("/joe-project", flags)
            descriptors.callback(os.close, descriptor)
            for name in parts[2:]:
                descriptor = os.open(name, flags, dir_fd=descriptor)
                descriptors.callback(os.close, descriptor)
            mount(f"/proc/self/fd/{descriptor}", "/workspace", None,
                  MountFlag.BIND | MountFlag.RECURSIVE)
        hidden = MountFlag.READ_ONLY | MountFlag.NO_SUID | MountFlag.NO_DEVICES
        mount("tmpfs", "/joe-project", "tmpfs", hidden, "size=4096,mode=000")
        private = MountFlag.NO_SUID | MountFlag.NO_DEVICES
        mount("tmpfs", "/tmp", "tmpfs", private, "size=1g,mode=1777")
        mount("tmpfs", "/dev/shm", "tmpfs", private, "size=64m,mode=1777")
        os.chdir("/workspace")


def execute():
    request = json.load(sys.stdin)
    protection = request["protection"]
    WorkspaceMount(protection["workspace"]).apply()
    mounts = ([ProtectedMount(path, Access.READ_ONLY) for path in protection["read_only"]]
              + [ProtectedMount(path, Access.HIDDEN) for path in protection["hidden"]])
    for protection in sorted(mounts, key=lambda entry: len(pathlib.PurePosixPath(entry.path).parts)):
        protection.apply()
    command = request["command"]
    arguments = ["/usr/bin/setpriv", "--no-new-privs",
                 "--bounding-set=-all,+dac_override", "--inh-caps=-all", "--ambient-caps=-all",
                 "--clear-groups", "--", "/usr/bin/env", "-i", "--",
                 *[f"{key}={value}" for key, value in command["environment"].items()],
                 "/usr/bin/python3", "-I", "/usr/local/libexec/joe-session.py", "command",
                 command["program"], *command["args"]]
    descriptor = os.open("/dev/null", os.O_RDONLY)
    os.dup2(descriptor, 0)
    os.close(descriptor)
    os.execve(arguments[0], arguments, {})


def run_command(arguments):
    completed = subprocess.run(arguments, stdin=subprocess.DEVNULL)
    if pathlib.Path("/tmp/sccache.sock").is_socket():
        with suppress(subprocess.SubprocessError):
            subprocess.run(["/usr/local/bin/sccache", "--stop-server"],
                           stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=30, check=True)
    sys.exit(completed.returncode if completed.returncode >= 0 else 128 - completed.returncode)


@dataclass
class Job:
    identifier: str
    command: dict
    protection: dict
    phase: Phase = Phase.STARTING
    process: asyncio.subprocess.Process | None = None

    def stop(self):
        match self.phase:
            case Phase.STARTING:
                self.phase = Phase.STOPPING
            case Phase.RUNNING:
                self.phase = Phase.STOPPING
                try:
                    os.killpg(self.process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            case _:
                pass

    def emit(self, message):
        emit({"event": "command", "id": self.identifier, "message": message})

    async def output(self, stream, name):
        data = await stream.read(8192)
        while data:
            self.emit({"event": "output", "stream": name,
                       "data": base64.b64encode(data).decode("ascii")})
            data = await stream.read(8192)

    async def run(self):
        try:
            await asyncio.to_thread(refresh_metadata)
            self.process = await asyncio.create_subprocess_exec(
                "/usr/bin/unshare", "--mount", "--pid", "--ipc", "--net", "--fork", "--kill-child=KILL",
                "--mount-proc", "--propagation", "private", "--",
                "/usr/bin/python3", "-I", "/usr/local/libexec/joe-session.py", "execute",
                env={}, cwd="/",
                stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE, start_new_session=True,
            )
            previous = self.phase
            self.phase = Phase.RUNNING
            if previous is Phase.STOPPING:
                self.stop()
            self.process.stdin.write(json.dumps({"command": self.command,
                                                "protection": self.protection}).encode())
            await self.process.stdin.drain()
            self.process.stdin.close()
            await asyncio.gather(
                self.output(self.process.stdout, "stdout"),
                self.output(self.process.stderr, "stderr"),
                self.process.wait(),
            )
            self.emit({"event": "exited",
                       "exit_code": self.process.returncode if self.process.returncode >= 0 else None})
        except Exception as error:
            self.stop()
            if self.process is not None:
                await self.process.wait()
            self.emit({"event": "failed", "error": str(error)})
        finally:
            self.phase = Phase.COMPLETE


async def serve():
    if os.isatty(sys.stdin.fileno()):
        tty.setraw(sys.stdin.fileno())
    reader = asyncio.StreamReader(limit=1024 * 1024)
    protocol = asyncio.StreamReaderProtocol(reader)
    await asyncio.get_running_loop().connect_read_pipe(lambda: protocol, sys.stdin.buffer)
    jobs = {}
    tasks = set()
    emit({"event": "ready"})
    line = await reader.readline()
    while line:
        request = json.loads(line)
        identifier = request["id"]
        match request["request"]:
            case "run":
                job = Job(identifier, request["command"], request["protection"])
                jobs[identifier] = job
                task = asyncio.create_task(job.run())
                tasks.add(task)
                task.add_done_callback(tasks.discard)
                task.add_done_callback(lambda _, identifier=identifier: jobs.pop(identifier, None))
            case "cancel":
                job = jobs.get(identifier)
                if job is not None:
                    job.stop()
        line = await reader.readline()
    for job in list(jobs.values()):
        job.stop()
    await asyncio.gather(*tasks)


if __name__ == "__main__":
    match sys.argv[1:]:
        case ["execute"]:
            execute()
        case ["command", *arguments]:
            run_command(arguments)
        case []:
            asyncio.run(serve())
        case _:
            raise ValueError("Unknown guest operation")

import os
import pathlib
import socket
import subprocess
import sys
import time


def denied(operation):
    try:
        operation()
    except OSError:
        pass
    else:
        raise AssertionError("Host operation succeeded")


mode = os.environ["JOE_SANDBOX_FIXTURE"]
root = pathlib.Path.cwd()
marker = pathlib.Path(os.environ["JOE_SANDBOX_MARKER"])

match mode:
    case "pipes":
        sys.stdout.buffer.write(b"o" * (256 * 1024))
        sys.stderr.buffer.write(b"e" * (256 * 1024))
    case "overflow":
        sys.stdout.buffer.write(b"x" * (17 * 1024 * 1024))
    case "tree":
        marker.write_text("parent")
        subprocess.Popen(
            [sys.executable, "-c", "import pathlib, time, sys\np = pathlib.Path(sys.argv[1])\nwhile True:\n p.write_text(str(time.monotonic_ns()))\n time.sleep(0.02)", str(marker.with_suffix(".child"))],
            start_new_session=True,
        )
        time.sleep(300)
    case "environment-child":
        assert "JOE_INHERITED_SECRET" not in os.environ
        assert "SSH_AUTH_SOCK" not in os.environ
        assert os.environ["HOME"] == "/workspace"
    case "temporary-storage":
        temporary = pathlib.Path(os.environ["TMPDIR"])
        (temporary / "storage-fixture" / ".turbo-code").mkdir(parents=True)
        (temporary / "storage-fixture" / ".turbo-code" / "data.mdb").write_text("fixture")
        alias = temporary / "saved-session-alias"
        alias.symlink_to(root / ".turbo-code")
        for directory in [root / ".turbo-code", alias]:
            denied(lambda: (directory / "secret").read_bytes())
            denied(lambda: (directory / "secret").write_text("overwrite"))
    case "boundary":
        outside = pathlib.Path(os.environ["JOE_OUTSIDE"])
        assert root == pathlib.Path("/workspace")
        assert os.environ["HOME"] == str(root)
        assert "JOE_INHERITED_SECRET" not in os.environ
        assert "SSH_AUTH_SOCK" not in os.environ
        denied(lambda: (outside / "secret").read_bytes())
        denied(lambda: (outside / "secret").write_text("changed"))
        denied(lambda: (root / "escape/secret").write_text("changed"))
        denied(lambda: os.link(outside / "secret", root / "hardlink"))
        denied(lambda: os.link(root / "input", root / "new-link"))
        denied(lambda: (root / "input").rename(outside / "moved"))
        for name in [".git/config", ".GIT/config", ".agents/rules", ".codex/config", "readonly/file", "READONLY/file"]:
            denied(lambda: (root / name).write_text("changed"))
        denied(lambda: (root / "readonly").rename(root / "formerly-readonly"))
        denied(lambda: (root / ".turbo-code/config").read_bytes())
        host, port = os.environ["JOE_ENDPOINT"].rsplit(":", 1)
        denied(lambda: socket.create_connection((host, int(port)), timeout=1))
        denied(lambda: socket.create_connection(("1.1.1.1", 443), timeout=1))
        denied(lambda: (pathlib.Path("/usr/local/libexec/joe-guest")).write_text("changed"))
        (root / "allowed").write_text("allowed")
        child = subprocess.run([sys.executable, "-c", "import pathlib, sys\ntry:\n pathlib.Path(sys.argv[1]).write_text('changed')\nexcept OSError:\n pass\nelse:\n raise AssertionError('Escaped session')", str(outside / "secret")], start_new_session=True)
        assert child.returncode == 0

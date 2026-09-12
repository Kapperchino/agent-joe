set -eu
mount -t virtiofs joe-workspace /workspace
mount -t tmpfs -o size=256m,nosuid,nodev tmpfs /tmp
cd /workspace
exec /usr/bin/python3 -I /usr/local/libexec/joe-session.py

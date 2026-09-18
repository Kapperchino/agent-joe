set -eu
mount -t virtiofs joe-project /joe-project
mount -t virtiofs joe-cache /cache
mount -t tmpfs -o size=256m,nosuid,nodev tmpfs /tmp
mkdir -p /dev/shm
cd /
exec /usr/bin/python3 -I /usr/local/libexec/joe-session.py

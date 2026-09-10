set -eu
mount -t virtiofs joe-workspace /workspace
mount -t tmpfs -o size=256m,nosuid,nodev tmpfs /tmp
cd /workspace
exec /bin/sh "/workspace/target/.joe/tmp/$1/command"

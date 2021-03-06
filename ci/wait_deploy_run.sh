#!/bin/sh
set -e
set -u

t=$(cat logname || mktemp)

echo -n Going to wait for '$HOME/shurik_new\b' being closed...
while /usr/sbin/lsof | egrep -q "$HOME/shurik_new\b"; do
    echo -n ' '
    sleep 1;
    echo -n .
done;
echo ok

echo -n Sleeping for 10 seconds for running process to finish its job...
sleep 10;
echo ok

echo Going to kill running process...
if pgrep shurik >/dev/null 2>&1; then
    echo -n found running, killing...
    pkill shurik
fi;
echo ok
echo -n Moving new file over old...
mv shurik_new shurik
echo ok

echo Launching new file!

RUST_BACKTRACE=1
export RUST_BACKTRACE

RUST_LOG=info
export RUST_LOG

LD_LIBRARY_PATH="$PWD"
export LD_LIBRARY_PATH

./shurik
echo This point is reached after process is terminated

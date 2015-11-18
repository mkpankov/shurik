#!/bin/sh
set -e
set -u

echo -n Going to wait for "$HOME/shurik_new\b" being closed...
while lsof | egrep -q "$HOME/shurik_new\b"; do
    echo -n ' '
    sleep 1;
    echo -n .
done;
echo ok

echo Going to kill running process...
if pgrep shurik; then
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
./shurik
echo This point is reached after process is terminated

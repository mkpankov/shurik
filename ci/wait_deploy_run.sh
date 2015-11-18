#!/bin/sh
set -e
set -u

echo 1
while lsof | egrep -q "$HOME/shurik_new\b"; do
    echo 2
    sleep 1;
    echo 3
done;
echo 4

echo 5
sleep 10
echo 6

echo 7
if pgrep shurik; then
    echo 7a
    pkill shurik
    echo 7b
fi;
echo 8
mv shurik_new shurik
echo 9

echo 10
RUST_BACKTRACE=1
export RUST_BACKTRACE
./shurik
echo 11

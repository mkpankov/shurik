#!/bin/sh
set -e
set -u

echo 1
while lsof 2>/dev/null | egrep -q "\b$PWD/shurik_new\b" >/dev/null 2>&1; do
    echo 2
    sleep 1;
    echo 3
done;
echo 4

echo 5
sleep 10
echo 6

echo 7
pkill shurik
echo 8
mv shurik_new shurik
echo 9

echo 10
RUST_BACKTRACE=1
export RUST_BACKTRACE
./shurik
echo 11

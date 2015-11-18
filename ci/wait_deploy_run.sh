#!/bin/sh
set -e
set -u

while ! lsof | egrep -q "\b$PWD/shurik_new\b"; do
    sleep 1;
done;

sleep 10

pkill shurik
mv shurik_new shurik

RUST_BACKTRACE=1
export RUST_BACKTRACE
./shurik

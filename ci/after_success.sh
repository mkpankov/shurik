#!/bin/sh

set -e
set -u
set -x

. ./environment

scp ./target/debug/shurik user@host
ssh user@host 'pkill shurik || nohup "RUST_BACKTRACE=1 ./shurik"'

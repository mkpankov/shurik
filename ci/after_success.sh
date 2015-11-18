#!/bin/sh

set -e
set -u
set -x

. ./environment

HOST='user@host'

scp ./ci/run.sh "$HOST:"
scp ./target/debug/shurik "$HOST:"
ssh "$HOST" 'ssh-keyscan gitlab.host > ~/.ssh/known_hosts'

ssh "$HOST" 'pkill shurik || sh -c "nohup \"./run.sh\" > shurik.out 2>&1 < /dev/null &"'

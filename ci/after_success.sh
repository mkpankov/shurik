#!/bin/sh

set -e
set -u
set -x

. ./environment

HOST='user@host'

ssh "$HOST" 'pkill shurik'

scp ./ci/run.sh "$HOST:"
scp ./target/debug/shurik "$HOST:"
ssh "$HOST" 'ssh-keyscan gitlab.host > ~/.ssh/known_hosts'

ssh "$HOST" 'sh -c "nohup \"./run.sh\" > shurik.out 2>&1 < /dev/null &"'

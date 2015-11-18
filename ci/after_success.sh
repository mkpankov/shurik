#!/bin/sh

set -e
set -u
set -x

. ./environment

HOST='user@host'


scp ./ci/run.sh "$HOST:"
scp ./target/debug/shurik "$HOST:shurik_new"
ssh "$HOST" 'ssh-keyscan gitlab.host > ~/.ssh/known_hosts'

ssh "$HOST" 'sh -c "nohup \"sleep 10 && pkill shurik && mv shurik_new shurik && ./run.sh\" >shurik.out 2>&1"'

#!/bin/sh

set -e
set -u
set -x

. ./environment

if [ "$RUN_TYPE" = "deploy" ]; then
    HOST='user@host'

    ssh "$HOST" 'rm -f shurik_new'

    scp ./ci/wait_deploy_run.sh "$HOST:"
    scp ./target/debug/shurik "$HOST:shurik_new"
    scp /lib/libssl.so.1.0.0 "$HOST:"
    scp /lib/libcrypto.so.1.0.0 "$HOST:"

    ssh "$HOST" 'sh -c "mktemp > logname"'
    ssh "$HOST" 'sh -c "nohup \"./wait_deploy_run.sh\" > $(cat logname) 2>&1 < /dev/null &"'
fi;

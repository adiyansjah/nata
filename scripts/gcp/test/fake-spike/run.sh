#!/bin/bash
sleep "0.0$((RANDOM % 5))"
case "$1" in
  tc*) cargo --version >/dev/null 2>&1 && { echo "baseline passes (1s)"; echo "CAUGHT   $1  [tests]"; } || echo "BASELINE FAILS: no toolchain" ;;
  px*|name*) echo "baseline passes (1s)"; echo "[ 1/4] CAUGHT by tests: $1"; echo "1 of 1 mutants caught; 0 problem(s)" ;;
  none) [ "${FAKE_LIST:-}" = controlfail ] && { echo "BASELINE FAILS"; exit 1; }; echo "baseline passes (1s)" ;;
  none_unused) echo "baseline passes (1s)" ;;
  survivor*) echo "baseline passes (1s)"; echo "SURVIVED  $1  [no layer failed]" ;;
  bad*) echo "baseline passes (1s)"; echo "INVALID  $1  does not compile" ;;
  hang*) sleep 30 ;;
  *) echo "baseline passes (1s)"; echo "CAUGHT   $1  [tests]" ;;
esac

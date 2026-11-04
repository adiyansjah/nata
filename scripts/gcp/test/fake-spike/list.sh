#!/bin/bash
# The fake spike's mutants: FAKE_LIST selects the scenario.
case "${FAKE_LIST:-clean}" in
  clean) for i in $(seq 1 24); do echo "ok$i"; done ;;
  survivor) for i in $(seq 1 12); do echo "ok$i"; done; echo "survivor1" ;;
  invalid) for i in $(seq 1 6); do echo "ok$i"; done; echo "bad1" ;;
  controlfail) for i in $(seq 1 5); do echo "ok$i"; done ;;
  toolchain) for i in $(seq 1 4); do echo "tc$i"; done ;;
  prefixed) for i in $(seq 1 4); do echo "px$i"; done; echo "name with spaces"; echo "name with slash (a/b path)" ;;
  empty) : ;;
esac

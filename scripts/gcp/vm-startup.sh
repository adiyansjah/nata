#!/bin/bash
# Startup script of the Spot VM; it runs on every boot, including after a recreate. It prepares the persistent
# data disk, installs the few OS packages, watches for the preemption notice, and starts the job when the
# orchestrator has delivered the source (/mnt/work/START). The job itself resumes from its step markers.
exec > >(tee -a /var/log/nata-startup.log) 2>&1
set -u
MNT=/mnt/work
DEV=/dev/disk/by-id/google-work
# A PEER of a multi-VM run (orchestrate.sh --peers K, attribute nata-peer=1) has no data disk and runs no job: the driver VM of the run
# reaches it over the private network with the run's throwaway key and starts the processes itself. It gets the user `nata` with
# passwordless sudo (the partition test drops its traffic with iptables): the peer has no service account and no scopes, so root on it
# reaches nothing outside it. Nothing else happens on a peer.
if [ "$(curl -s -m 5 -H 'Metadata-Flavor: Google' http://metadata.google.internal/computeMetadata/v1/instance/attributes/nata-peer 2>/dev/null)" = 1 ]; then
  id nata > /dev/null 2>&1 || useradd -m -s /bin/bash nata
  echo 'nata ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/nata-peer && chmod 440 /etc/sudoers.d/nata-peer
  command -v iptables > /dev/null || { DEBIAN_FRONTEND=noninteractive apt-get update -qq; DEBIAN_FRONTEND=noninteractive apt-get install -y -qq iptables; }
  touch /var/run/nata-peer-ready
  exit 0
fi
for _ in $(seq 1 90); do [ -e "$DEV" ] && break; sleep 2; done
blkid "$DEV" > /dev/null 2>&1 || mkfs.ext4 -q -F "$DEV"
mkdir -p "$MNT"
mountpoint -q "$MNT" || mount -o discard,defaults "$DEV" "$MNT"
id nata > /dev/null 2>&1 || useradd -m -s /bin/bash nata
chown nata:nata "$MNT"
export DEBIAN_FRONTEND=noninteractive
command -v cc > /dev/null || { DEBIAN_FRONTEND=noninteractive apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential curl git ca-certificates pkg-config; }

# Spot VMs get a notice about 30 seconds before they are reclaimed: tell the job, so it can leave clean markers.
nohup bash -c 'while sleep 3; do
  [ "$(curl -s -H "Metadata-Flavor: Google" http://metadata.google.internal/computeMetadata/v1/instance/preempted)" = TRUE ] \
    && { pkill -TERM -u nata -f job.sh; sync; break; }
done' > /dev/null 2>&1 &

# Wait for the orchestrator to deliver the source, then run the job (once) as the unprivileged user.
for _ in $(seq 1 600); do [ -f "$MNT/START" ] && break; sleep 2; done
# Jobs that need root-only setup get it here, because the job itself runs as the unprivileged user `nata`.
# The `perf` job profiles with perf: install it and allow unprivileged profiling (Debian sets perf_event_paranoid to 3).
case "$(cat "$MNT/JOBNAME" 2> /dev/null)" in perf|timing) ;; *) false ;; esac && {
  command -v perf > /dev/null || DEBIAN_FRONTEND=noninteractive apt-get install -y -qq linux-perf || DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "linux-perf-$(uname -r | cut -d. -f1-2)"
  sysctl -w kernel.perf_event_paranoid=1 kernel.kptr_restrict=0 > /dev/null 2>&1 || true
}
# The Iceberg commit jobs build a Python venv for the independent-reader check (pyiceberg); the job runs without sudo, so install it here.
case "$(cat "$MNT/JOBNAME" 2> /dev/null)" in iceberg|iceberg-smoke|icebergspikes|icebergpolaris|slice4bench|slice4close) python3 -m venv --help > /dev/null 2>&1 && python3 -c "import ensurepip" 2> /dev/null || DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3-venv ;; esac
case "$(cat "$MNT/JOBNAME" 2> /dev/null)" in debt-rss)
  DEBIAN_FRONTEND=noninteractive apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq heaptrack python3-venv ;;
esac
case "$(cat "$MNT/JOBNAME" 2> /dev/null)" in wirecapture)
  DEBIAN_FRONTEND=noninteractive apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq postgresql postgresql-client python3-venv default-jdk-headless ;;
esac
case "$(cat "$MNT/JOBNAME" 2> /dev/null)" in pgwirebench|item35)
  DEBIAN_FRONTEND=noninteractive apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq postgresql postgresql-client linux-perf valgrind python3-venv default-jdk-headless
  echo 'nata ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/nata-pgwirebench
  chmod 440 /etc/sudoers.d/nata-pgwirebench
  sysctl -w kernel.perf_event_paranoid=1 kernel.kptr_restrict=0 > /dev/null 2>&1 || true ;;
esac
# The network-exchange job (verification-plan item 28, spikes/net-exchange/gcp-check.sh) builds Linux network namespaces with `tc netem`: that needs
# root, iproute2 (ip, tc), ethtool (segmentation offload off, so that netem loss is per packet) and the sch_netem module. The job runs as the
# unprivileged `nata`, so it is given passwordless sudo on THIS VM only: the VM is a throwaway with no service account and no scopes
# (orchestrate.sh creates it with --no-service-account --no-scopes), so root on it reaches nothing outside it.
case "$(cat "$MNT/JOBNAME" 2> /dev/null)" in netexchsim) ;; *) false ;; esac && {
  command -v tc > /dev/null && command -v ethtool > /dev/null || { DEBIAN_FRONTEND=noninteractive apt-get update -qq; DEBIAN_FRONTEND=noninteractive apt-get install -y -qq iproute2 ethtool; }  # a fresh image has no package lists: without the update the first netexchsim run silently had no ethtool
  modprobe sch_netem 2> /dev/null || true
  echo 'nata ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/nata-netexch && chmod 440 /etc/sudoers.d/nata-netexch
}
# The Kani jobs (and the memory measurements of hashops and timing) record peak memory with GNU time.
case "$(cat "$MNT/JOBNAME" 2> /dev/null)" in kani*|timing|hashops|spillbench|aggref|aggref-scale|heavy) [ -x /usr/bin/time ] || DEBIAN_FRONTEND=noninteractive apt-get install -y -qq time ;; esac
# The cgroup check (verification-plan item 9) starts the scan in systemd scopes with a CPU quota, which needs root: passwordless sudo for
# `nata` on this throwaway VM (no service account, no scopes), for this job only.
case "$(cat "$MNT/JOBNAME" 2> /dev/null)" in cgroupcheck) echo 'nata ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/nata-cgroup && chmod 440 /etc/sudoers.d/nata-cgroup ;; esac
# The other CBMC back ends for the solver comparison of the division proofs.
case "$(cat "$MNT/JOBNAME" 2> /dev/null)" in kani-solvers) for pk in z3 cvc5; do command -v $pk > /dev/null || DEBIAN_FRONTEND=noninteractive apt-get install -y -qq $pk || echo "vm-startup: $pk not installable"; done ;; esac
if [ -f "$MNT/START" ] && ! pgrep -u nata -f "scripts/gcp/job.sh" > /dev/null; then
  su - nata -c "mkdir -p $MNT/logs && cd $MNT && WORK=$MNT nohup bash src/scripts/gcp/job.sh >> $MNT/logs/job.out 2>&1 &"
fi

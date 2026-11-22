#!/usr/bin/env bash
# Headless Arch Linux VM (QEMU/KVM) for testing Atlas packages and the GUI.
#
#   scripts/arch-vm/vm.sh up        # create (first run) and boot the VM
#   scripts/arch-vm/vm.sh ssh [cmd] # run a command in the VM as user "atlas"
#   scripts/arch-vm/vm.sh put SRC DST / get SRC DST
#   scripts/arch-vm/vm.sh down      # power off
#
# State lives in $ATLAS_VM_DIR (default ~/vm/atlas-arch). Needs qemu-system-x86_64,
# cloud-localds (cloud-image-utils) and access to /dev/kvm.
set -euo pipefail
dir=${ATLAS_VM_DIR:-$HOME/vm/atlas-arch}
port=${ATLAS_VM_SSH_PORT:-2222}
cpus=${ATLAS_VM_CPUS:-4}
mem=${ATLAS_VM_MEM:-4096}
image_url=https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2
ssh_opts=(-i "$dir/id_ed25519" -p "$port" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
  -o LogLevel=ERROR -o ConnectTimeout=5)

create() {
  mkdir -p "$dir"
  cd "$dir"
  [[ -f arch.qcow2 ]] || {
    curl -fL -o arch.qcow2 "$image_url"
    curl -fL -o arch.qcow2.SHA256 "$image_url.SHA256"
    sed 's/ .*/  arch.qcow2/' arch.qcow2.SHA256 | sha256sum -c
  }
  [[ -f id_ed25519 ]] || ssh-keygen -q -t ed25519 -N '' -f id_ed25519
  cat > user-data <<EOF
#cloud-config
users:
  - name: atlas
    groups: [wheel]
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - $(cat id_ed25519.pub)
EOF
  printf 'instance-id: atlas-arch-1\nlocal-hostname: atlas-arch\n' > meta-data
  cloud-localds seed.iso user-data meta-data
  qemu-img create -q -f qcow2 -b arch.qcow2 -F qcow2 disk.qcow2 40G
}

case ${1:-} in
  up)
    [[ -f $dir/disk.qcow2 ]] || create
    if [[ -f $dir/qemu.pid ]] && kill -0 "$(cat "$dir/qemu.pid")" 2>/dev/null; then
      echo "VM already running (pid $(cat "$dir/qemu.pid"))"; exit 0
    fi
    qemu-system-x86_64 -enable-kvm -cpu host -smp "$cpus" -m "$mem" \
      -drive "file=$dir/disk.qcow2,if=virtio" -drive "file=$dir/seed.iso,if=virtio,media=cdrom" \
      -nic "user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:$port-:22" \
      -device virtio-rng-pci -display none -serial "file:$dir/serial.log" \
      -daemonize -pidfile "$dir/qemu.pid"
    for _ in $(seq 120); do
      ssh "${ssh_opts[@]}" atlas@127.0.0.1 true 2>/dev/null && { echo "VM up on port $port"; exit 0; }
      sleep 2
    done
    echo "VM did not answer on ssh; see $dir/serial.log" >&2; exit 1 ;;
  ssh) shift; exec ssh "${ssh_opts[@]}" atlas@127.0.0.1 "$@" ;;
  put) exec scp -q -r -i "$dir/id_ed25519" -P "$port" -o StrictHostKeyChecking=no \
         -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "$2" "atlas@127.0.0.1:$3" ;;
  get) exec scp -q -r -i "$dir/id_ed25519" -P "$port" -o StrictHostKeyChecking=no \
         -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "atlas@127.0.0.1:$2" "$3" ;;
  down)
    ssh "${ssh_opts[@]}" atlas@127.0.0.1 sudo systemctl poweroff 2>/dev/null || true
    for _ in $(seq 30); do
      [[ -f $dir/qemu.pid ]] && kill -0 "$(cat "$dir/qemu.pid")" 2>/dev/null || { rm -f "$dir/qemu.pid"; exit 0; }
      sleep 1
    done
    kill "$(cat "$dir/qemu.pid")"; rm -f "$dir/qemu.pid" ;;
  *) sed -n '2,10p' "$0"; exit 2 ;;
esac

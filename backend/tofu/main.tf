provider "libvirt" {
  uri = var.libvirt_uri
}


locals {
  # Read rather than passed in: the NixOS module bakes this guest's address into
  # the image at build time, and a flake cannot see the environment .env lives
  # in. One tracked file is the only way the image and the hypervisor's view of
  # the same VM cannot disagree.
  spec = jsondecode(file("${path.module}/../lab.json"))

  name = local.spec.name

  gateway_ipv4 = "${local.spec.net.subnet_prefix}.${local.spec.net.gateway_host}"
  ipv4         = "${local.spec.net.subnet_prefix}.${local.spec.net.host}"
  netmask      = cidrnetmask("${local.spec.net.subnet_prefix}.0/${local.spec.net.prefix_length}")

  # Derived from the name so it survives a rebuild, and so nothing has to
  # remember it. The name differs from every distro-lab lab, so the address
  # differs from every distro-lab MAC.
  mac_address = join(":", [
    "52", "54", "00",
    substr(md5(local.name), 0, 2),
    substr(md5(local.name), 2, 2),
    substr(md5(local.name), 4, 2),
  ])

  # The root is a thin overlay on the base, so the base is read-only for as long
  # as the root names it and can never be rewritten in place. `just image` names
  # each base after the store path it was built from and records that path here;
  # a new build lands beside the old one, this reads a different backing path,
  # and terraform_data.base below forces the root to be recreated against it.
  #
  # Before the first `just image` there is nothing to read. Falling back to the
  # unstamped name keeps a first `just apply` from failing on a missing file,
  # and `just doctor` reports the real problem.
  stamp_file = "${var.storage_path}/images/${local.name}-base.store-path"

  base_path = (
    fileexists(local.stamp_file)
    ? "${var.storage_path}/images/${local.name}-base-${substr(
      basename(trimspace(file(local.stamp_file))), 0, 8
    )}.qcow2"
    : "${var.storage_path}/images/${local.name}-base.qcow2"
  )

  work_volume_name = "${local.name}-work.qcow2"

  # The host side of the guest's /var/lib/ui-box-state. The idle markers the
  # guest publishes and the keepalive the wake path writes both live here.
  state_share_path = "${var.storage_path}/storage/${local.name}/state"
  state_share_tag  = "uiboxstate"

  cpu_shares = try(local.spec.cpu_shares, 1024)
  title      = try(local.spec.title, "ui-box graphical test backend")
}


resource "libvirt_pool" "images" {
  name = var.pool
  type = "dir"

  target = {
    path = "${var.storage_path}/images"
  }

  # build stays off so the directory keeps the ownership `just apply` gave it.
  # A pool libvirt builds is root's, and both `just image` and virtiofsd need
  # to write into that tree as the invoking user.
  create = {
    build     = false
    start     = true
    autostart = true
  }

  # Dropping the pool object must never take the base images with it.
  destroy = {
    delete = false
  }
}


# The provider does not mark backing_store as forcing replacement: it plans a
# changed base as an in-place update and then refuses it at apply time with
# "Storage volumes cannot be updated", which wedges the stack. Carry the base
# path in something whose replacement can be triggered and hang the root off
# it, so `just image` followed by `just apply` recreates the overlay.
resource "terraform_data" "base" {
  triggers_replace = local.base_path
}


resource "libvirt_volume" "root" {
  name     = "${local.name}.qcow2"
  pool     = libvirt_pool.images.name
  capacity = local.spec.disk_size_bytes

  target = {
    format = {
      type = "qcow2"
    }
  }

  # The overlay declares disk_size_bytes, larger than the base it sits on:
  # qcow2 reads past the end of a backing file as zeroes, and the guest's
  # growfs takes the root filesystem out to the full size on first boot. That
  # is why the base is never resized.
  backing_store = {
    path = local.base_path

    format = {
      type = "qcow2"
    }
  }

  lifecycle {
    replace_triggered_by = [terraform_data.base]
  }
}


# Separate from the root on purpose: the root is replaced every time a new base
# is built, and this is the disk that has to survive that. prevent_destroy is
# what makes the difference load-bearing rather than incidental — a changed
# work_disk_bytes would otherwise plan as a replacement and take the checkout,
# the node_modules and the cargo target with it, silently.
resource "libvirt_volume" "work" {
  name     = local.work_volume_name
  pool     = libvirt_pool.images.name
  capacity = local.spec.work_disk_bytes

  target = {
    format = {
      type = "qcow2"
    }
  }

  lifecycle {
    prevent_destroy = true
  }
}


resource "libvirt_domain" "vm" {
  name  = local.name
  type  = "kvm"
  title = local.title

  # Started by bin/ui-box-wake on demand and stopped by bin/ui-box-idle-stop,
  # so tofu neither boots it nor cares that something else did.
  running   = false
  autostart = false

  memory      = local.spec.memory_mib
  memory_unit = "MiB"

  current_memory      = local.spec.current_memory_mib
  current_memory_unit = "MiB"

  vcpu         = local.spec.vcpu
  vcpu_current = local.spec.vcpu_current

  # virtiofs forces this: a share needs the guest's RAM to be shareable, and
  # memfd with shared access is the backing that provides it.
  memory_backing = {
    memory_source = {
      type = "memfd"
    }

    memory_access = {
      mode = "shared"
    }
  }

  features = {
    acpi = true
    apic = {}
  }

  # Left to itself libvirt hands the guest one single-core socket per vCPU and
  # QEMU invents a private 16 MiB L3 for each of them. The host is a single-CCD
  # 9800X3D where all eight cores sit under one 96 MiB V-Cache, so that
  # topology is not a simplification of the machine but the opposite of it: the
  # guest scheduler is told migrating a thread costs nothing, and anything that
  # sizes itself from topology reads a dozen machines where there is one. One
  # socket of `vcpu` cores puts the whole guest back under a single shared L3.
  #
  # The topology describes the hotplug ceiling rather than the boot count:
  # libvirt requires sockets * cores * threads to equal <vcpu>, and
  # vcpu_current only decides how many of those come online.
  #
  # threads stays 1 because nothing here pins a vCPU to a host thread.
  cpu = {
    mode = "host-passthrough"

    topology = {
      sockets = 1
      cores   = local.spec.vcpu
      threads = 1
    }

    # Without this QEMU invents cache sizes to go with the topology it
    # invented, and cache-blocked code sizes its tiles for a 16 MiB L3 that
    # does not exist. Passthrough reports what the host has: 48 KiB 12-way L1d,
    # 1 MiB L2, 96 MiB L3.
    #
    # topoext has to be asked for by name. AMD publishes cache sizes in CPUID
    # leaf 0x8000001D, which is only readable when topoext is set, and QEMU
    # leaves it clear here even under host-passthrough — passthrough without it
    # leaves the guest with no cache sysfs at all, which is worse than the
    # invented numbers it replaces.
    cache = {
      mode = "passthrough"
    }

    features = [
      {
        name   = "topoext"
        policy = "require"
      }
    ]
  }

  cpu_tune = {
    shares = local.cpu_shares
  }

  os = {
    type         = "hvm"
    type_arch    = "x86_64"
    type_machine = "q35"

    firmware = "efi"

    firmware_info = {
      features = [
        {
          name    = "enrolled-keys"
          enabled = "no"
        },
        {
          name    = "secure-boot"
          enabled = "no"
        }
      ]
    }

    boot_devices = [
      {
        dev = "hd"
      }
    ]
  }

  devices = {
    disks = [
      {
        source = {
          volume = {
            pool   = libvirt_volume.root.pool
            volume = libvirt_volume.root.name
          }
        }

        target = {
          dev = "vda"
          bus = "virtio"
        }

        # Without this the guest's weekly fstrim is thrown away at the
        # virtio-blk layer: the trim reaches the driver, the driver has no
        # discard to issue, and the qcow2 keeps every cluster the guest has
        # stopped using. Unmapping a cluster of an overlay writes a zero marker
        # rather than a hole, so the base underneath still cannot show through.
        driver = {
          type    = "qcow2"
          discard = "unmap"
        }
      },
      {
        source = {
          volume = {
            pool   = libvirt_volume.work.pool
            volume = libvirt_volume.work.name
          }
        }

        target = {
          dev = "vdb"
          bus = "virtio"
        }

        driver = {
          type    = "qcow2"
          discard = "unmap"
        }
      },
    ]

    interfaces = [
      {
        type = "network"

        model = {
          type = "virtio"
        }

        mac = {
          address = local.mac_address
        }

        # By name. The network object itself belongs to distro-lab's shared
        # stack, and this guest holds no reservation in it — the image carries
        # the static address that puts it on local.ipv4.
        source = {
          network = {
            network = var.network
          }
        }
      }
    ]

    filesystems = [
      {
        access_mode = "passthrough"

        driver = {
          type = "virtiofs"
        }

        # libvirt spawns virtiofsd itself and offers no XML for its migration
        # flags, so <binary path> is the only seam. The wrapper ships beside
        # this stack rather than coming from the environment: a VM that cannot
        # restore its own managed save is broken in a way no .env should be
        # able to cause by omission.
        binary = {
          path = abspath("${path.module}/../bin/ui-box-virtiofsd")
        }

        source = {
          mount = {
            dir = local.state_share_path
          }
        }

        target = {
          dir = local.state_share_tag
        }
      }
    ]

    mem_balloon = {
      model               = "virtio"
      free_page_reporting = "on"
      auto_deflate        = "off"

      stats = {
        period = 10
      }
    }

    channels = [
      {
        source = {
          unix = {}
        }

        target = {
          virt_io = {
            name = "org.qemu.guest_agent.0"
          }
        }
      }
    ]

    graphics = [
      {
        vnc = {
          auto_port = true
          listen    = "127.0.0.1"
        }
      }
    ]

    serials = [
      {
        target = {
          type = "isa-serial"
          port = 0
        }
      }
    ]

    consoles = [
      {
        target = {
          type = "serial"
          port = 0
        }
      }
    ]
  }

  destroy = {
    shutdown = {
      timeout = 120
    }
  }

  lifecycle {
    ignore_changes = [running]

    precondition {
      condition     = local.spec.memory_mib >= local.spec.current_memory_mib
      error_message = "memory_mib is the balloon ceiling and must be at least current_memory_mib."
    }

    precondition {
      condition     = local.spec.vcpu >= local.spec.vcpu_current
      error_message = "vcpu is the hotplug ceiling and must be at least vcpu_current."
    }

    precondition {
      condition     = contains(["managedsave", "shutdown", "none"], local.spec.idle.action)
      error_message = "idle.action must be one of managedsave, shutdown, none."
    }

    # A save costs the ceiling, not the balloon. Guest RAM is memfd with shared
    # access — the virtiofs share is what forces that backing — and a read
    # fault on a shared shmem mapping allocates. So when managedsave walks the
    # RAM block to write it out, it materialises the whole of memory_mib
    # however little the guest is actually holding.
    #
    # Measured on this host with a distro-lab lab: 24576 MiB with 1.2 GiB
    # resident spent 29s climbing to 17.4 GiB of shmem before the OOM killer
    # took QEMU, which leaves the VM hard-stopped with no save file — a power
    # cut dressed as a suspend. The same lab at 8192 saved in 2.9s.
    #
    # Raise this only alongside host memory, and never above what `just doctor`
    # reports as spendable.
    precondition {
      condition     = local.spec.idle.action != "managedsave" || local.spec.memory_mib <= 12288
      error_message = "idle.action managedsave requires memory_mib at or below 12288; a save materialises the ceiling and a larger one OOM-kills the domain it is saving."
    }

    precondition {
      condition     = local.spec.net.host > 1 && local.spec.net.host < 255
      error_message = "net.host must be between 2 and 254."
    }

    precondition {
      condition     = local.ipv4 != local.gateway_ipv4
      error_message = "net.host claims the gateway address."
    }

    # The checkout, node_modules and cargo target live here and have to outlive
    # every new base image.
    precondition {
      condition     = local.spec.work_disk_bytes > 0
      error_message = "work_disk_bytes must be set; the root is replaced on every new base and cannot hold the checkout."
    }
  }
}

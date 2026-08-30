output "name" {
  value = libvirt_domain.vm.name
}

output "ipv4" {
  value = local.ipv4
}

output "mac_address" {
  value = local.mac_address
}

output "disk_path" {
  value = libvirt_volume.root.path
}

output "work_disk_path" {
  value = libvirt_volume.work.path
}

output "base_path" {
  value = local.base_path
}

output "state_share" {
  value = {
    path = local.state_share_path
    tag  = local.state_share_tag
  }
}

output "pool" {
  value = libvirt_pool.images.name
}

output "network" {
  value = var.network
}

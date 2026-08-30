variable "libvirt_uri" {
  type = string
}

# Outside the repo, always. A path: flake reference copies its whole root into
# the nix store verbatim, and .gitignore has no say in that, so a 100 GiB qcow2
# living under the checkout would be copied on every evaluation.
variable "storage_path" {
  type = string

  validation {
    condition     = startswith(var.storage_path, "/")
    error_message = "storage_path must be absolute."
  }
}

# ui-box's own dir pool. distro-lab owns the images pool on the same host and
# declares it from its own state; two stacks declaring one pool fight over it.
variable "pool" {
  type    = string
  default = "ui-box"
}

# Referenced by name and never declared here: distro-lab's shared stack owns
# the dlab network, its DHCP reservations and its DNS entries. This guest is
# not in them and does not need to be — the image carries its address.
variable "network" {
  type    = string
  default = "dlab"
}

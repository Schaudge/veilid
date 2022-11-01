resource "digitalocean_droplet" "veilid-runner-1" {
  image = "debian-11-x64"
  name = "veilid-runner-1"
  region = "nyc1"
  size = "s-1vcpu-512mb-10gb"
  ssh_keys = [
    data.digitalocean_ssh_key.ssh_key.id
  ]

  connection {
    host = self.ipv4_address
    user = "root"
    type = "ssh"
    private_key = file(var.pvt_key)
    timeout = "2m"
  }

  provisioner "remote-exec" {
    inline = [
      "apt-get update",
      "apt-get install python3-apt -y"
    ]
  }

  provisioner "local-exec" {
    command = <<EOF
ANSIBLE_HOST_KEY_CHECKING=False ansible-playbook -u root \
  -i '${self.ipv4_address},' \
  --private-key ${var.pvt_key} \
  -e "regkey=${var.reg_key} ci_server_url=${var.ci_server_url} runner_name=${var.runner_name}" \
  docker-install.yaml
EOF
  }
}

output "droplet_ip_address" {
  value = digitalocean_droplet.veilid-runner-1
}

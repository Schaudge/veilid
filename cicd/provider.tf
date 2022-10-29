terraform {
  required_providers {
    digitalocean = {
      source = "digitalocean/digitalocean"
      version = "~> 2.0"
    }
  }
}

variable "do_token" {}
variable "ssh_key" {}
variable "pvt_key" {}
variable "reg_key" {}
variable "ci_server_url" {}
variable "runner_name" {}

provider "digitalocean" {
  token = var.do_token
}

data "digitalocean_ssh_key" "ssh_key" {
  name = var.ssh_key 
}

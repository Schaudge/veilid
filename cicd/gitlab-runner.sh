#!/usr/bin/env bash


install () {
  docker run -d --name gitlab-runner --restart always \
    -v /srv/gitlab-runner/config:/etc/gitlab-runner \
    -v /var/run/docker.sock:/var/run/docker.sock \
    gitlab/gitlab-runner:latest
}

register () {
  docker run --rm -it \
    -v /srv/gitlab-runner/config:/etc/gitlab-runner gitlab/gitlab-runner register
}

case $1 in
  install)
    install
    ;;

  register)
    register
    ;;

esac

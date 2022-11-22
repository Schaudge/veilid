#!/usr/bin/env bash


install () {
  docker run -d --name gitlab-runner --restart always \
    -v /srv/gitlab-runner/config:/etc/gitlab-runner \
    -v /var/run/docker.sock:/var/run/docker.sock \
    --hostname="gitlab-runner" \
    --network="host" \
    gitlab/gitlab-runner:latest
}

register () {

  docker run --rm -it \
    -v /srv/gitlab-runner/config:/etc/gitlab-runner \
    -v /tmp/gitlab-runner:/tmp/gitlab-runner \
    --network="host" \
    gitlab/gitlab-runner register \
    --config /etc/gitlab-runner/config.toml \
    --template-config /tmp/gitlab-runner/template.config.toml \
    --non-interactive \
    --executor "docker" \
    --docker-image alpine:latest \
    --url "${CI_SERVER_URL}" \
    --registration-token "${REGISTRATION_TOKEN}" \
    --description "${RUNNER_NAME}" \
    --tag-list "amd64,linux"
}

case $1 in
  install)
    install
    ;;

  register)
    register
    ;;

esac

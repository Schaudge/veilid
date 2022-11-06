#!/usr/bin/env bash

docker run -d --restart always \
  --privileged \
  --name earthly-buildkit \
  -t -p 8372:8372 \
  -v earthly-tmp:/tmp/earthly:rw \
  -v /var/run/docker.sock:/var/run/docker.sock \
  --env BUILDKIT_TCP_TRANSPORT_ENABLED=true \
  earthly/buildkitd:v0.6.28

#!/usr/bin/env bash

docker run -d --restart always \
  --privileged \
  --name earthly-buildkit \
  --network host \
  -t -p 8372:8372 \
  -v earthly-tmp:/tmp/earthly:rw \
  -v /var/run/docker.sock:/var/run/docker.sock \
  --env BUILDKIT_TCP_TRANSPORT_ENABLED=true \
  --env CNI_MTU=1500 \
  earthly/buildkitd:v0.6.28

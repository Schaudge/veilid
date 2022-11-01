#!/usr/bin/env bash

docker run -d --restart always \
  --privileged \
  --name earthly-buildkit \
  -p 8372:8372 \
  -t -v earthly-tmp:/tmp/earthly:rw \
  --env BUILDKIT_TCP_TRANSPORT_ENABLED=true \
  earthly/buildkitd:v0.6.28

#!/usr/bin/env bash

wget https://github.com/earthly/earthly/releases/download/v0.6.27/earthly-linux-amd64 \
  -O /usr/local/bin/earthly
chmod +x /usr/local/bin/earthly
/usr/local/bin/earthly bootstrap

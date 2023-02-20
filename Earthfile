VERSION 0.6

# Start with older Ubuntu to ensure GLIBC symbol versioning support for older linux
# Ensure we are using an amd64 platform because some of these targets use cross-platform tooling
FROM --platform amd64 ubuntu:16.04

# Install build prerequisites
deps-base:
    RUN apt-get -y update
    RUN apt-get install -y iproute2 curl build-essential cmake libssl-dev openssl file git pkg-config libdbus-1-dev libdbus-glib-1-dev libgirepository1.0-dev libcairo2-dev checkinstall unzip capnproto-c++-0.10.2

# Install protoc
deps-protoc:
    FROM +deps-base
    COPY scripts/earthly/install_protoc.sh /
    RUN /bin/bash /install_protoc.sh; rm /install_protoc.sh

# Install Rust
deps-rust:
    FROM +deps-protoc
    ENV RUSTUP_HOME=/usr/local/rustup
    ENV CARGO_HOME=/usr/local/cargo
    ENV PATH=/usr/local/cargo/bin:$PATH
    RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y -c clippy --no-modify-path --profile minimal 
    RUN chmod -R a+w $RUSTUP_HOME $CARGO_HOME; \
        rustup --version; \
        cargo --version; \
        rustc --version;
    # ARM64 Linux
    RUN rustup target add aarch64-unknown-linux-gnu
    # Android
    RUN rustup target add aarch64-linux-android
    RUN rustup target add armv7-linux-androideabi
    RUN rustup target add i686-linux-android
    RUN rustup target add x86_64-linux-android
    # WASM
    RUN rustup target add wasm32-unknown-unknown

# Install Linux cross-platform tooling
deps-cross:
    FROM +deps-rust
    RUN apt-get install -y gcc-aarch64-linux-gnu curl unzip

# Install android tooling
deps-android:
    FROM +deps-cross
    RUN apt-get install -y openjdk-9-jdk-headless
    RUN mkdir /Android; mkdir /Android/Sdk
    RUN curl -o /Android/cmdline-tools.zip https://dl.google.com/android/repository/commandlinetools-linux-9123335_latest.zip
    RUN cd /Android; unzip /Android/cmdline-tools.zip
    RUN yes | /Android/cmdline-tools/bin/sdkmanager --sdk_root=/Android/Sdk build-tools\;33.0.1 ndk\;25.1.8937393 cmake\;3.22.1 platform-tools platforms\;android-33
    RUN apt-get clean
    
# Just linux build not android
deps-linux:
    FROM +deps-cross
    RUN apt-get clean

# Code + Linux deps
code-linux:
    FROM +deps-linux
    COPY --dir .cargo external files scripts veilid-cli veilid-core veilid-server veilid-tools veilid-flutter veilid-wasm Cargo.lock Cargo.toml /veilid
    RUN cat /veilid/scripts/earthly/cargo-linux/config.toml >> /veilid/.cargo/config.toml
    WORKDIR /veilid

# Code + Linux + Android deps
code-android:
    FROM +deps-android
    COPY --dir .cargo external files scripts veilid-cli veilid-core veilid-server veilid-tools veilid-flutter veilid-wasm Cargo.lock Cargo.toml /veilid
    RUN cat /veilid/scripts/earthly/cargo-linux/config.toml >> /veilid/.cargo/config.toml
    RUN cat /veilid/scripts/earthly/cargo-android/config.toml >> /veilid/.cargo/config.toml
    WORKDIR /veilid

# Clippy only
clippy:
    FROM +code-linux
    RUN cargo clippy

# Build
build-linux-amd64:
    FROM +code-linux
    RUN cargo build --target x86_64-unknown-linux-gnu --release
    SAVE ARTIFACT ./target/x86_64-unknown-linux-gnu AS LOCAL ./target/artifacts/x86_64-unknown-linux-gnu

build-linux-arm64:
    FROM +code-linux
    RUN cargo build --target aarch64-unknown-linux-gnu --release
    SAVE ARTIFACT ./target/aarch64-unknown-linux-gnu AS LOCAL ./target/artifacts/aarch64-unknown-linux-gnu

build-android:
    FROM +code-android
    WORKDIR /veilid/veilid-core
    ENV PATH=$PATH:/Android/Sdk/ndk/25.1.8937393/toolchains/llvm/prebuilt/linux-x86_64/bin/
    RUN cargo build --target aarch64-linux-android --release
    RUN cargo build --target armv7-linux-androideabi --release
    RUN cargo build --target i686-linux-android --release
    RUN cargo build --target x86_64-linux-android --release
    WORKDIR /veilid
    SAVE ARTIFACT ./target/aarch64-linux-android AS LOCAL ./target/artifacts/aarch64-linux-android
    SAVE ARTIFACT ./target/armv7-linux-androideabi AS LOCAL ./target/artifacts/armv7-linux-androideabi
    SAVE ARTIFACT ./target/i686-linux-android AS LOCAL ./target/artifacts/i686-linux-android
    SAVE ARTIFACT ./target/x86_64-linux-android AS LOCAL ./target/artifacts/x86_64-linux-android

# Unit tests
unit-tests-linux-amd64:
    FROM +code-linux
    RUN cargo test --target x86_64-unknown-linux-gnu --release

unit-tests-linux-arm64:
    FROM +code-linux
    RUN cargo test --target aarch64-unknown-linux-gnu --release

# Package 
package-linux-amd64:
    FROM +build-linux-amd64
    #################################
    ### DEBIAN DPKG .DEB FILES
    #################################
    COPY --dir package /veilid
    # veilid-server
    RUN /veilid/package/debian/earthly_make_veilid_server_deb.sh amd64 x86_64-unknown-linux-gnu
    SAVE ARTIFACT --keep-ts /dpkg/out/*.deb AS LOCAL ./target/packages/
    # veilid-cli
    RUN /veilid/package/debian/earthly_make_veilid_cli_deb.sh amd64 x86_64-unknown-linux-gnu
    # save artifacts
    SAVE ARTIFACT --keep-ts /dpkg/out/*.deb AS LOCAL ./target/packages/
    
package-linux-arm64:
    FROM +build-linux-arm64
    #################################
    ### DEBIAN DPKG .DEB FILES
    #################################
    COPY --dir package /veilid
    # veilid-server
    RUN /veilid/package/debian/earthly_make_veilid_server_deb.sh arm64 aarch64-unknown-linux-gnu
    SAVE ARTIFACT --keep-ts /dpkg/out/*.deb AS LOCAL ./target/packages/
    # veilid-cli
    RUN /veilid/package/debian/earthly_make_veilid_cli_deb.sh arm64 aarch64-unknown-linux-gnu
    # save artifacts
    SAVE ARTIFACT --keep-ts /dpkg/out/*.deb AS LOCAL ./target/packages/

package-linux:
    BUILD +package-linux-amd64
    BUILD +package-linux-arm64

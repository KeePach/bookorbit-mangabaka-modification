FROM rust:1.90-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY testdata ./testdata
# The crate registry and the build folder outlive the build, so a rebuild recompiles only what
# changed. The binary is copied out because a cache mount is not part of the image.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=bookwalker-target,sharing=locked \
    cargo build --release --locked && cp target/release/bookwalker-provider /bookwalker-provider \
    && mkdir /empty-data

# Static musl binary; rustls carries its own root certificates, so the image needs nothing else.
FROM scratch
COPY --from=build /bookwalker-provider /bookwalker-provider
# Where the catalog is saved. Owned by the user the provider runs as, so a fresh named volume
# mounted here starts out writable.
COPY --from=build --chown=65534:65534 /empty-data /data
USER 65534:65534
EXPOSE 8080
VOLUME /data
ENTRYPOINT ["/bookwalker-provider"]

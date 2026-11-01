FROM rust:bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY web ./web
RUN cargo build --release --locked && \
    mkdir -p /out/var/lib/alpendns /out/var/cache/alpendns/lists

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/alpendns /usr/local/bin/alpendns
COPY --from=build --chown=65532:65532 /out/var/lib/alpendns /var/lib/alpendns
COPY --from=build --chown=65532:65532 /out/var/cache/alpendns/lists /var/cache/alpendns/lists
COPY --chown=0:0 packaging/container/alpendns.toml /etc/alpendns/alpendns.toml
USER 65532:65532
VOLUME ["/etc/alpendns", "/var/lib/alpendns", "/var/cache/alpendns"]
EXPOSE 53/udp 53/tcp 8053 9153
ENTRYPOINT ["/usr/local/bin/alpendns"]
CMD ["-c", "/etc/alpendns/alpendns.toml"]

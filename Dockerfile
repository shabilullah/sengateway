FROM rust:1.88-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
COPY migrations migrations
COPY src src
COPY static static
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl openssl && rm -rf /var/lib/apt/lists/*
RUN mkdir /data && chown 65532:65532 /data
COPY --from=build /src/target/release/sengateway /usr/local/bin/sengateway
USER 65532:65532
VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["sengateway"]

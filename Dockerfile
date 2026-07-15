ARG RUST_VERSION=1.97.0
ARG CARGO_CHEF_VERSION=0.1.77

FROM rust:${RUST_VERSION}-slim-trixie AS chef
ARG CARGO_CHEF_VERSION
WORKDIR /app
RUN cargo install --locked cargo-chef@${CARGO_CHEF_VERSION}

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin cururu

FROM gcr.io/distroless/cc-debian13:nonroot AS runtime
WORKDIR /app
COPY --from=builder /app/target/release/cururu ./cururu
ENTRYPOINT ["./cururu"]

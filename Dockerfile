FROM rust:1-alpine3.16

# Update the crates.io index
RUN cargo search libc --limit 1 && chmod -R 777 /usr/local/cargo

RUN apk add --no-cache make musl-dev

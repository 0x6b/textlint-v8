# syntax=docker/dockerfile:1.7
FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder

WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f

COPY --from=builder /src/target/release/textlint-v8 /usr/local/bin/textlint-v8

EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/textlint-v8"]
CMD ["--mcp"]

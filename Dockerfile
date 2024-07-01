FROM rust:1.79-alpine as builder

RUN apk add pcc-libs-dev musl-dev pkgconfig

WORKDIR /usr/src/dockerprom
COPY src ./src
COPY Cargo.lock .
COPY Cargo.toml .

RUN cargo install --path .

###

FROM alpine:3.20.1

COPY --from=builder /usr/local/cargo/bin/dockerprom /dockerprom

ENV LISTEN_ADDR=[::]:3000
EXPOSE 3000

STOPSIGNAL SIGKILL

ENTRYPOINT ["/dockerprom"]

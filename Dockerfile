# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

# Java 25 runtime. More runtimes can be added later as /opt/java/<ver> + ABMPT_JAVA_<ver>.
FROM eclipse-temurin:25-jre
RUN groupadd --system --gid 10001 abmpt \
 && useradd --system --uid 10001 --gid abmpt --home-dir /data --no-create-home abmpt \
 && mkdir -p /data && chown abmpt:abmpt /data
COPY --from=build /src/target/release/abmpt /usr/local/bin/abmpt

ENV ABMPT_DATA_DIR=/data \
    ABMPT_LISTEN=0.0.0.0:3307 \
    ABMPT_GAME_PORTS=65000-65100 \
    ABMPT_RCON_PORT_BASE=35000 \
    ABMPT_JAVA_25=/opt/java/openjdk/bin/java \
    ABMPT_JAVA_DEFAULT=25 \
    HOME=/data

USER abmpt
VOLUME ["/data"]
EXPOSE 3307 65000-65100
ENTRYPOINT ["/usr/local/bin/abmpt"]
CMD ["serve"]

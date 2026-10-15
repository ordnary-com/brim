# The notes service as a native executable in a small image:
#
#   docker build -t notes examples/http
#   docker run -p 8080:8080 notes
#
# The build stage installs a released brim with native builds (Linux
# x86_64) and compiles the service; the image only holds the executable.
FROM --platform=linux/amd64 debian:bookworm-slim AS build
ARG BRIM_VERSION=0.2.0
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://raw.githubusercontent.com/ordnary-com/brim/main/install.sh \
    | BRIM_VERSION=$BRIM_VERSION BRIM_INSTALL=/usr/local/bin sh
WORKDIR /src
COPY notes.brim .
RUN brim build --release notes.brim -o /notes

FROM --platform=linux/amd64 debian:bookworm-slim
COPY --from=build /notes /usr/local/bin/notes
ENV HOST=0.0.0.0 PORT=8080
EXPOSE 8080
USER nobody
CMD ["notes"]

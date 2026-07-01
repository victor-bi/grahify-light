FROM rust:1-bookworm AS build

ARG CODEX_CLI_VERSION=0.142.5

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        nodejs \
        npm \
        python3 \
        python3-pip \
        python3-venv \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g "@openai/codex@${CODEX_CLI_VERSION}"

RUN python3 -m venv /opt/graphify \
    && /opt/graphify/bin/pip install --no-cache-dir --upgrade pip \
    && /opt/graphify/bin/pip install --no-cache-dir graphifyy

ENV PATH="/opt/graphify/bin:${PATH}"

WORKDIR /workspace/source
COPY Cargo.toml Cargo.lock README.md AGENTS.md LICENSE ./
COPY src ./src
COPY docs ./docs
COPY tests ./tests
COPY benchmarking/Cargo.toml benchmarking/Cargo.lock ./benchmarking/
COPY benchmarking/.gitignore benchmarking/README.md benchmarking/Dockerfile ./benchmarking/
COPY benchmarking/src ./benchmarking/src

RUN cargo build --release --bin graphify-light \
    && install -m 755 target/release/graphify-light /usr/local/bin/graphify-light

WORKDIR /workspace/source/benchmarking
RUN cargo build --release \
    && install -m 755 target/release/graphify-bench /usr/local/bin/graphify-bench

WORKDIR /workspace/source
ENTRYPOINT ["graphify-bench"]
CMD ["--repo", "/workspace/source", "--out", "/workspace/out"]

FROM communicatio/rust:nightly

USER root
RUN dnf install -y libpq-devel && dnf clean all

WORKDIR /app
COPY . .
RUN chown -R rust: .

USER rust
RUN cargo install --path .

CMD ["events"]

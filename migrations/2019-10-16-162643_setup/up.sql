-- Your SQL goes here
CREATE TABLE events (
  sequence bigserial PRIMARY KEY,
  version int NOT NULL,
  data jsonb NOT NULL,
  timestamp timestamptz NOT NULL default now()
);

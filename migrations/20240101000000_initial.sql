-- Create scanner_progress table
CREATE TABLE scanner_progress (
    chain TEXT PRIMARY KEY,
    last_block BIGINT NOT NULL,
    last_timestamp TIMESTAMPTZ NOT NULL
);

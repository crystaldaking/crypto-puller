-- Create wallets table
CREATE TABLE wallets (
    id SERIAL PRIMARY KEY,
    chain TEXT NOT NULL,
    address TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(chain, address)
);

-- Optional: insert initial wallets (can be done via API)
-- INSERT INTO wallets (chain, address) VALUES ('Ethereum', '0x58b704065B7aFF3ED351052f8560019E05925023');

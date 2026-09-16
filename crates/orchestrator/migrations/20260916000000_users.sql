CREATE TABLE users (
    principal  TEXT PRIMARY KEY,
    name       TEXT,
    status     TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL
);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    principal  TEXT NOT NULL REFERENCES users(principal),
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE TABLE personal_tokens (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash TEXT NOT NULL UNIQUE,
    principal  TEXT NOT NULL REFERENCES users(principal),
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE workers (
    id             TEXT PRIMARY KEY,
    worker_type    TEXT NOT NULL,
    token          TEXT NOT NULL UNIQUE,
    status         TEXT NOT NULL DEFAULT 'starting',
    created_at     TEXT NOT NULL,
    last_heartbeat TEXT
);

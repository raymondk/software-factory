-- Spec 5: providers belong to the developer who added them. The token is stored as is: the orchestrator presents it.
CREATE TABLE providers (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    owner      TEXT NOT NULL REFERENCES users(principal),
    name       TEXT NOT NULL,
    url        TEXT NOT NULL,
    token      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (owner, name)
);

-- The provider id instead of its former name in the config.
ALTER TABLE workers DROP COLUMN provider;
ALTER TABLE workers ADD COLUMN provider INTEGER NOT NULL DEFAULT 0;

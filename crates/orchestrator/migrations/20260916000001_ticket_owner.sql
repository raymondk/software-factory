-- The human who created the ticket, by principal. Informational; nothing checks it.
ALTER TABLE tickets ADD COLUMN owner TEXT;

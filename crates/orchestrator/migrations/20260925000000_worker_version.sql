-- The worker binary's version, reported when it registers.
ALTER TABLE workers ADD COLUMN version TEXT;

-- Store the moderator's user ID for ban/kick actions.
-- Used to update embed mentions when a moderator leaves the server.
ALTER TABLE memberships ADD COLUMN moderator_id TEXT;

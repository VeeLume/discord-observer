-- Store the inviter's user ID so we can @mention them in embeds.
ALTER TABLE memberships ADD COLUMN inviter_id TEXT;

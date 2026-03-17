-- Store who invited the member (displayed on leave embeds).
ALTER TABLE memberships ADD COLUMN inviter_name TEXT;

-- Track the message ID of the join/leave embed so we can link between them.
ALTER TABLE memberships ADD COLUMN embed_channel_id TEXT;
ALTER TABLE memberships ADD COLUMN embed_message_id TEXT;

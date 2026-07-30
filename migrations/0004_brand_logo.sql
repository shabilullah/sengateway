ALTER TABLE settings ADD COLUMN logo_content_type TEXT CHECK(logo_content_type IN ('image/png','image/jpeg','image/webp'));
ALTER TABLE settings ADD COLUMN logo_data BLOB CHECK(logo_data IS NULL OR length(logo_data) <= 1048576);

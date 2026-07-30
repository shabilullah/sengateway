ALTER TABLE coupons ADD COLUMN code_ciphertext BLOB;
ALTER TABLE coupons ADD COLUMN code_nonce BLOB CHECK(code_nonce IS NULL OR length(code_nonce) = 12);
ALTER TABLE coupon_templates ADD COLUMN unlimited_devices INTEGER NOT NULL DEFAULT 0 CHECK(unlimited_devices IN (0,1));
ALTER TABLE coupon_templates ADD COLUMN never_expires INTEGER NOT NULL DEFAULT 0 CHECK(never_expires IN (0,1));
ALTER TABLE coupons ADD COLUMN unlimited_devices INTEGER NOT NULL DEFAULT 0 CHECK(unlimited_devices IN (0,1));
ALTER TABLE coupons ADD COLUMN never_expires INTEGER NOT NULL DEFAULT 0 CHECK(never_expires IN (0,1));

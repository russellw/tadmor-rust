-- 000016_self_organization (down)

DROP INDEX organizations_is_self_uq;
ALTER TABLE organizations DROP COLUMN is_self;

-- 000015_cash_flow (down)

ALTER TABLE accounts DROP CONSTRAINT accounts_is_cash_asset_check;
ALTER TABLE accounts DROP COLUMN cash_flow_activity;
ALTER TABLE accounts DROP COLUMN is_cash;

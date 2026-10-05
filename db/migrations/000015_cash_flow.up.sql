-- 000015_cash_flow: account metadata for the cash-flow statement.
--
-- The statement of cash flows needs two facts the chart of accounts did not
-- record:
--
--   * which accounts *are* cash (and cash equivalents) — the statement
--     explains the change in their combined balance;
--   * which activity section — operating, investing, or financing — a
--     non-cash balance-sheet account's movements belong to.
--
-- cash_flow_activity is only consulted for asset/liability/equity accounts:
-- revenue and expense movements reach the statement through net income, which
-- is always operating. The default is 'operating' (right for the working-
-- capital accounts a small chart is mostly made of); equity accounts are
-- backfilled to 'financing'. Fixed-asset or loan accounts should be set to
-- 'investing' / 'financing' when they are created.

ALTER TABLE accounts
    ADD COLUMN is_cash boolean NOT NULL DEFAULT false,
    ADD COLUMN cash_flow_activity text NOT NULL DEFAULT 'operating'
        CONSTRAINT accounts_cash_flow_activity_check
        CHECK (cash_flow_activity IN ('operating', 'investing', 'financing'));

-- Only an asset can be cash.
ALTER TABLE accounts
    ADD CONSTRAINT accounts_is_cash_asset_check
    CHECK (NOT is_cash OR account_type = 'asset');

-- Backfill existing charts: asset accounts whose names say cash or bank are
-- flagged as cash; equity movements are financing activity.
UPDATE accounts SET is_cash = true
 WHERE account_type = 'asset' AND name ~* '\m(cash|bank)\M';

UPDATE accounts SET cash_flow_activity = 'financing'
 WHERE account_type = 'equity';

-- 000008_grni: a Goods-Received-Not-Invoiced clearing account.
--
-- An inventory receipt credits this account (Dr inventory / Cr GRNI); the
-- matching purchase bill line then debits it (Dr GRNI / Cr A/P), so GRNI nets to
-- zero once goods received have been invoiced. It is a liability: a balance here
-- represents goods received but not yet billed.
INSERT INTO accounts (code, name, account_type, is_postable) VALUES
    ('2150', 'Goods Received Not Invoiced', 'liability', true)
ON CONFLICT (code) DO NOTHING;

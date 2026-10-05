-- 000006_purchasing (down)
DROP VIEW IF EXISTS ap_aging;
DROP VIEW IF EXISTS purchase_bill_balances;

DROP TABLE IF EXISTS bill_applications;
DROP TABLE IF EXISTS supplier_payments;
DROP TABLE IF EXISTS purchase_bill_lines;
DROP TABLE IF EXISTS purchase_bills;
DROP TABLE IF EXISTS suppliers;

DROP FUNCTION IF EXISTS trg_bill_application_check();
DROP FUNCTION IF EXISTS trg_purchase_bill_recompute();

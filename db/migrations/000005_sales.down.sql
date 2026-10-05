-- 000005_sales (down)
DROP VIEW IF EXISTS ar_aging;
DROP VIEW IF EXISTS sales_invoice_balances;

DROP TABLE IF EXISTS payment_applications;
DROP TABLE IF EXISTS customer_payments;
DROP TABLE IF EXISTS sales_invoice_lines;
DROP TABLE IF EXISTS sales_invoices;
DROP TABLE IF EXISTS customers;
DROP TABLE IF EXISTS products;
DROP TABLE IF EXISTS tax_codes;
DROP TABLE IF EXISTS payment_terms;

DROP FUNCTION IF EXISTS trg_payment_application_check();
DROP FUNCTION IF EXISTS trg_sales_invoice_recompute();

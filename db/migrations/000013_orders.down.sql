-- 000013_orders (down)

DROP VIEW purchase_order_fulfilment;
DROP VIEW purchase_order_line_fulfilment;
DROP VIEW sales_order_fulfilment;
DROP VIEW sales_order_line_fulfilment;

DROP TRIGGER purchase_bill_lines_order_check ON purchase_bill_lines;
DROP FUNCTION trg_bill_line_order_check();
DROP TRIGGER sales_invoice_lines_order_check ON sales_invoice_lines;
DROP FUNCTION trg_invoice_line_order_check();

ALTER TABLE purchase_bill_lines DROP COLUMN order_line_id;
ALTER TABLE sales_invoice_lines DROP COLUMN order_line_id;

DROP TABLE purchase_order_lines;
DROP FUNCTION trg_purchase_order_recompute();
DROP TABLE purchase_orders;

DROP TABLE sales_order_lines;
DROP FUNCTION trg_sales_order_recompute();
DROP TABLE sales_orders;

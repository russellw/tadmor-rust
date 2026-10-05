-- 000020_exact_division (down)

CREATE OR REPLACE VIEW stock_on_hand AS
SELECT sm.product_id,
       sm.warehouse_id,
       sum(sm.quantity)   AS qty_on_hand,
       sum(sm.total_cost) AS value_on_hand,
       CASE WHEN sum(sm.quantity) <> 0
            THEN round(sum(sm.total_cost) / sum(sm.quantity), 4)
            ELSE 0 END    AS avg_unit_cost
FROM stock_movements sm
GROUP BY sm.product_id, sm.warehouse_id;

CREATE OR REPLACE VIEW stock_valuation AS
SELECT sm.product_id,
       sum(sm.quantity)   AS qty_on_hand,
       sum(sm.total_cost) AS value_on_hand,
       CASE WHEN sum(sm.quantity) <> 0
            THEN round(sum(sm.total_cost) / sum(sm.quantity), 4)
            ELSE 0 END    AS avg_unit_cost
FROM stock_movements sm
GROUP BY sm.product_id;

ALTER TABLE sales_invoice_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_price * tax_rate / 100, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_price, 4)
                                               + round(quantity * unit_price * tax_rate / 100, 4));
ALTER TABLE purchase_bill_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_cost * tax_rate / 100, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_cost, 4)
                                               + round(quantity * unit_cost * tax_rate / 100, 4));
ALTER TABLE sales_credit_note_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_price * tax_rate / 100, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_price, 4)
                                               + round(quantity * unit_price * tax_rate / 100, 4));
ALTER TABLE purchase_credit_note_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_cost * tax_rate / 100, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_cost, 4)
                                               + round(quantity * unit_cost * tax_rate / 100, 4));
ALTER TABLE sales_order_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_price * tax_rate / 100, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_price, 4)
                                               + round(quantity * unit_price * tax_rate / 100, 4));
ALTER TABLE purchase_order_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_cost * tax_rate / 100, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_cost, 4)
                                               + round(quantity * unit_cost * tax_rate / 100, 4));

DROP FUNCTION div_round4(numeric, numeric);

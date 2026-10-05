-- 000020_exact_division: make every rounded quotient exact.
--
-- The spec (domain §2) defines tax and average cost as exact quotients rounded
-- to 4 places, half away from zero. Postgres numeric division does not compute
-- the exact quotient: it rounds it to a scale chosen from the operands'
-- magnitudes (as few as 12 places once the quotient reaches 10^4), so a later
-- round(..., 4) can round twice. For example 920907399.1189 / 123456789.0123
-- is 7.45934999999999995..., which Postgres yields as 7.45935000... and then
-- rounds to 7.4594 instead of 7.4593.
--
-- Tax divides by 100, which is the same as multiplying by 0.01, and numeric
-- multiplication is exact. Average cost divides by a quantity, so it goes
-- through div_round4, which rounds using integer division alone.

CREATE FUNCTION div_round4(n numeric, d numeric) RETURNS numeric
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    -- floor(|n/d| * 10^4 + 1/2), signed: div() truncates exactly.
    RETURN sign(n) * sign(d) * div(abs(n) * 20000 + abs(d), 2 * abs(d)) * 0.0001;

ALTER TABLE sales_invoice_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_price * tax_rate * 0.01, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_price, 4)
                                               + round(quantity * unit_price * tax_rate * 0.01, 4));
ALTER TABLE purchase_bill_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_cost * tax_rate * 0.01, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_cost, 4)
                                               + round(quantity * unit_cost * tax_rate * 0.01, 4));
ALTER TABLE sales_credit_note_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_price * tax_rate * 0.01, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_price, 4)
                                               + round(quantity * unit_price * tax_rate * 0.01, 4));
ALTER TABLE purchase_credit_note_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_cost * tax_rate * 0.01, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_cost, 4)
                                               + round(quantity * unit_cost * tax_rate * 0.01, 4));
ALTER TABLE sales_order_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_price * tax_rate * 0.01, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_price, 4)
                                               + round(quantity * unit_price * tax_rate * 0.01, 4));
ALTER TABLE purchase_order_lines
    ALTER COLUMN tax_amount SET EXPRESSION AS (round(quantity * unit_cost * tax_rate * 0.01, 4)),
    ALTER COLUMN line_total SET EXPRESSION AS (round(quantity * unit_cost, 4)
                                               + round(quantity * unit_cost * tax_rate * 0.01, 4));

CREATE OR REPLACE VIEW stock_on_hand AS
SELECT sm.product_id,
       sm.warehouse_id,
       sum(sm.quantity)   AS qty_on_hand,
       sum(sm.total_cost) AS value_on_hand,
       CASE WHEN sum(sm.quantity) <> 0
            THEN div_round4(sum(sm.total_cost), sum(sm.quantity))
            ELSE 0 END    AS avg_unit_cost
FROM stock_movements sm
GROUP BY sm.product_id, sm.warehouse_id;

CREATE OR REPLACE VIEW stock_valuation AS
SELECT sm.product_id,
       sum(sm.quantity)   AS qty_on_hand,
       sum(sm.total_cost) AS value_on_hand,
       CASE WHEN sum(sm.quantity) <> 0
            THEN div_round4(sum(sm.total_cost), sum(sm.quantity))
            ELSE 0 END    AS avg_unit_cost
FROM stock_movements sm
GROUP BY sm.product_id;

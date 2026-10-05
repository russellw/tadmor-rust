-- 000007_inventory (down)
DROP VIEW IF EXISTS stock_below_reorder;
DROP VIEW IF EXISTS stock_valuation;
DROP VIEW IF EXISTS stock_on_hand;

DROP TABLE IF EXISTS inventory_levels;
DROP TABLE IF EXISTS stock_movements;

DROP FUNCTION IF EXISTS trg_stock_movement_product_ok();

ALTER TABLE products
    DROP COLUMN IF EXISTS cogs_account_id,
    DROP COLUMN IF EXISTS inventory_account_id,
    DROP COLUMN IF EXISTS track_inventory;

DROP TABLE IF EXISTS warehouses;

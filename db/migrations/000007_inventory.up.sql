-- 000007_inventory: stock tracking built as an append-only movement ledger.
--
-- Model:
--   warehouses        physical (or logical) stock locations
--   products (+cols)   gain inventory flags and valuation accounts
--   stock_movements   the append-only ledger: every +/- to on-hand quantity
--   inventory_levels  per product/warehouse reorder settings
--
-- Like the GL, the movement ledger is the source of truth: quantity-on-hand and
-- inventory value are *derived* by summing movements (see the views), never
-- stored as a mutable balance. Each movement records the unit_cost at which it
-- occurred; the service layer decides issue costs per its valuation policy
-- (weighted-average, FIFO, ...) and the schema stays method-agnostic.

-- ---------------------------------------------------------------------------
-- Warehouses
-- ---------------------------------------------------------------------------

CREATE TABLE warehouses (
    id         int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    code       text        NOT NULL UNIQUE,               -- short location code, e.g. 'MAIN'
    name       text        NOT NULL,
    address_id int         REFERENCES addresses(id),
    is_active  boolean     NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON warehouses
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ---------------------------------------------------------------------------
-- Inventory attributes on the shared product catalog
-- ---------------------------------------------------------------------------

-- Not every product is stocked (services, fees). track_inventory gates whether
-- a product may appear in stock movements; the two accounts drive GL posting
-- (Dr inventory on receipt, Cr inventory / Dr COGS on issue) in the service layer.
ALTER TABLE products
    ADD COLUMN track_inventory      boolean NOT NULL DEFAULT false,
    ADD COLUMN inventory_account_id int     REFERENCES accounts(id),
    ADD COLUMN cogs_account_id      int     REFERENCES accounts(id);

-- ---------------------------------------------------------------------------
-- Stock movement ledger
-- ---------------------------------------------------------------------------

CREATE TABLE stock_movements (
    id               int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    product_id       int           NOT NULL REFERENCES products(id),
    warehouse_id     int           NOT NULL REFERENCES warehouses(id),
    movement_date    date          NOT NULL DEFAULT current_date,
    movement_type    text          NOT NULL
                                   CHECK (movement_type IN
                                       ('receipt', 'issue', 'adjustment',
                                        'transfer_in', 'transfer_out')),
    -- Signed: positive increases on-hand, negative decreases it.
    quantity         numeric(19,4) NOT NULL CHECK (quantity <> 0),
    unit_cost        numeric(19,4) NOT NULL DEFAULT 0 CHECK (unit_cost >= 0),
    total_cost       numeric(19,4) GENERATED ALWAYS AS (round(quantity * unit_cost, 4)) STORED,
    -- Optional, polymorphic link to the document that caused the movement
    -- (e.g. 'purchase_bill_line', 'sales_invoice_line'); no FK by design.
    source_type      text,
    source_id        int,
    -- GL hook, set by the service layer when the movement is posted.
    period_id        int           REFERENCES accounting_periods(id),
    journal_entry_id int           REFERENCES journal_entries(id),
    reference        text,
    notes            text,
    created_by       int           REFERENCES users(id),
    created_at       timestamptz   NOT NULL DEFAULT now(),
    -- The sign of the quantity must agree with the movement type; adjustments
    -- may go either way (shrinkage vs found stock).
    CONSTRAINT stock_movements_sign CHECK (
        (movement_type IN ('receipt', 'transfer_in') AND quantity > 0) OR
        (movement_type IN ('issue', 'transfer_out')  AND quantity < 0) OR
        (movement_type = 'adjustment')
    )
);

CREATE INDEX stock_movements_product_warehouse_idx ON stock_movements (product_id, warehouse_id);
CREATE INDEX stock_movements_warehouse_id_idx      ON stock_movements (warehouse_id);
CREATE INDEX stock_movements_movement_date_idx     ON stock_movements (movement_date);
CREATE INDEX stock_movements_source_idx            ON stock_movements (source_type, source_id);

-- A movement may only reference an inventory-tracked, active product.
CREATE FUNCTION trg_stock_movement_product_ok() RETURNS trigger AS $$
DECLARE
    v_track  boolean;
    v_active boolean;
BEGIN
    SELECT track_inventory, is_active INTO v_track, v_active
        FROM products WHERE id = NEW.product_id;
    IF NOT v_track THEN
        RAISE EXCEPTION 'product % is not inventory-tracked', NEW.product_id;
    END IF;
    IF NOT v_active THEN
        RAISE EXCEPTION 'product % is inactive', NEW.product_id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER stock_movements_product_ok
    BEFORE INSERT OR UPDATE ON stock_movements
    FOR EACH ROW EXECUTE FUNCTION trg_stock_movement_product_ok();

-- ---------------------------------------------------------------------------
-- Reorder settings (per product, per warehouse)
-- ---------------------------------------------------------------------------

-- Composite natural key: no synthetic id needed for a pure association row.
CREATE TABLE inventory_levels (
    product_id       int           NOT NULL REFERENCES products(id),
    warehouse_id     int           NOT NULL REFERENCES warehouses(id),
    reorder_point    numeric(19,4) NOT NULL DEFAULT 0 CHECK (reorder_point >= 0),
    reorder_quantity numeric(19,4) NOT NULL DEFAULT 0 CHECK (reorder_quantity >= 0),
    PRIMARY KEY (product_id, warehouse_id)
);

-- ---------------------------------------------------------------------------
-- Reporting convenience
-- ---------------------------------------------------------------------------

-- Quantity and value on hand per product per warehouse. avg_unit_cost is the
-- moving-average cost of what remains (value / quantity).
CREATE VIEW stock_on_hand AS
SELECT sm.product_id,
       sm.warehouse_id,
       sum(sm.quantity)   AS qty_on_hand,
       sum(sm.total_cost) AS value_on_hand,
       CASE WHEN sum(sm.quantity) <> 0
            THEN round(sum(sm.total_cost) / sum(sm.quantity), 4)
            ELSE 0 END    AS avg_unit_cost
FROM stock_movements sm
GROUP BY sm.product_id, sm.warehouse_id;

-- Total valuation per product across all warehouses.
CREATE VIEW stock_valuation AS
SELECT sm.product_id,
       sum(sm.quantity)   AS qty_on_hand,
       sum(sm.total_cost) AS value_on_hand,
       CASE WHEN sum(sm.quantity) <> 0
            THEN round(sum(sm.total_cost) / sum(sm.quantity), 4)
            ELSE 0 END    AS avg_unit_cost
FROM stock_movements sm
GROUP BY sm.product_id;

-- Product/warehouse combinations at or below their reorder point.
CREATE VIEW stock_below_reorder AS
SELECT il.product_id,
       il.warehouse_id,
       il.reorder_point,
       il.reorder_quantity,
       COALESCE(soh.qty_on_hand, 0) AS qty_on_hand
FROM inventory_levels il
LEFT JOIN stock_on_hand soh
       ON soh.product_id = il.product_id AND soh.warehouse_id = il.warehouse_id
WHERE il.reorder_point > 0
  AND COALESCE(soh.qty_on_hand, 0) <= il.reorder_point;

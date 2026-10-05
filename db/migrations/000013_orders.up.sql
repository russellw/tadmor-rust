-- 000013_orders: sales orders and purchase orders.
--
-- Model:
--   sales_orders / _lines       what a customer has agreed to buy from us
--   purchase_orders / _lines    what we have agreed to buy from a supplier
--
-- Orders are commercial documents, not accounting ones: they never post to the
-- GL. Their lifecycle is draft -> open (confirmed) -> closed / cancelled, and
-- fulfilment happens by other documents referencing order lines:
--
--   invoicing   sales_invoice_lines.order_line_id  -> sales_order_lines
--   billing     purchase_bill_lines.order_line_id  -> purchase_order_lines
--   shipping    stock_movements (issue,   source_type 'sales_order_line')
--   receiving   stock_movements (receipt, source_type 'purchase_order_line')
--
-- Like the GL and the stock ledger, fulfilment state is *derived* (see the
-- views), never stored: an order line's invoiced/shipped/billed/received
-- quantities are sums over the documents that reference it. Constraint
-- triggers keep those references honest (same party and currency, no
-- over-invoicing / over-billing of a line).

-- ---------------------------------------------------------------------------
-- Sales orders
-- ---------------------------------------------------------------------------

CREATE TABLE sales_orders (
    id                 int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    order_number       text        NOT NULL UNIQUE,
    customer_id        int         NOT NULL REFERENCES customers(id),
    order_date         date        NOT NULL,
    expected_ship_date date,
    currency_code      char(3)     NOT NULL REFERENCES currencies(code),
    -- draft: editable quote/entry; open: confirmed, may be fulfilled;
    -- closed: complete (or terminated early); cancelled: never happened.
    status             text        NOT NULL DEFAULT 'draft'
                                   CHECK (status IN ('draft', 'open', 'closed', 'cancelled')),
    -- Header money is maintained from the lines by trigger while in draft.
    subtotal           numeric(19,4) NOT NULL DEFAULT 0,
    tax_total          numeric(19,4) NOT NULL DEFAULT 0,
    total              numeric(19,4) NOT NULL DEFAULT 0,
    reference          text,                               -- e.g. the customer's PO number
    memo               text,
    created_by         int         REFERENCES users(id),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT sales_orders_total_consistent CHECK (total = subtotal + tax_total),
    CONSTRAINT sales_orders_ship_after_date  CHECK (expected_ship_date IS NULL OR expected_ship_date >= order_date)
);

CREATE INDEX sales_orders_customer_id_idx ON sales_orders (customer_id);
CREATE INDEX sales_orders_status_idx      ON sales_orders (status);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON sales_orders
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE sales_order_lines (
    id                 int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    order_id           int           NOT NULL REFERENCES sales_orders(id) ON DELETE CASCADE,
    line_no            int           NOT NULL,
    product_id         int           REFERENCES products(id),   -- NULL = free-form line
    description        text          NOT NULL,
    quantity           numeric(19,4) NOT NULL DEFAULT 1 CHECK (quantity > 0),
    unit_price         numeric(19,4) NOT NULL DEFAULT 0,
    revenue_account_id int           REFERENCES accounts(id),
    tax_code           text          REFERENCES tax_codes(code),
    tax_rate           numeric(7,4)  NOT NULL DEFAULT 0 CHECK (tax_rate >= 0),  -- rate snapshot
    line_subtotal numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_price, 4)) STORED,
    tax_amount    numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_price * tax_rate / 100, 4)) STORED,
    line_total    numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_price, 4)
                             + round(quantity * unit_price * tax_rate / 100, 4)) STORED,
    CONSTRAINT sales_order_lines_line_no_uq UNIQUE (order_id, line_no)
);

CREATE INDEX sales_order_lines_order_id_idx   ON sales_order_lines (order_id);
CREATE INDEX sales_order_lines_product_id_idx ON sales_order_lines (product_id);

CREATE FUNCTION trg_sales_order_recompute() RETURNS trigger AS $$
DECLARE
    v_order  int;
    v_status text;
BEGIN
    v_order := COALESCE(NEW.order_id, OLD.order_id);
    SELECT status INTO v_status FROM sales_orders WHERE id = v_order;
    IF v_status IS NULL OR v_status <> 'draft' THEN
        RETURN NULL;
    END IF;
    UPDATE sales_orders so SET
        subtotal  = COALESCE(t.sub, 0),
        tax_total = COALESCE(t.tax, 0),
        total     = COALESCE(t.tot, 0)
    FROM (
        SELECT sum(line_subtotal) AS sub, sum(tax_amount) AS tax, sum(line_total) AS tot
        FROM sales_order_lines WHERE order_id = v_order
    ) t
    WHERE so.id = v_order;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER sales_order_lines_recompute
    AFTER INSERT OR UPDATE OR DELETE ON sales_order_lines
    FOR EACH ROW EXECUTE FUNCTION trg_sales_order_recompute();

-- ---------------------------------------------------------------------------
-- Purchase orders (issued by us; our numbering, globally unique)
-- ---------------------------------------------------------------------------

CREATE TABLE purchase_orders (
    id                    int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    order_number          text        NOT NULL UNIQUE,
    supplier_id           int         NOT NULL REFERENCES suppliers(id),
    order_date            date        NOT NULL,
    expected_receipt_date date,
    currency_code         char(3)     NOT NULL REFERENCES currencies(code),
    status                text        NOT NULL DEFAULT 'draft'
                                      CHECK (status IN ('draft', 'open', 'closed', 'cancelled')),
    subtotal              numeric(19,4) NOT NULL DEFAULT 0,
    tax_total             numeric(19,4) NOT NULL DEFAULT 0,
    total                 numeric(19,4) NOT NULL DEFAULT 0,
    reference             text,
    memo                  text,
    created_by            int         REFERENCES users(id),
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT purchase_orders_total_consistent   CHECK (total = subtotal + tax_total),
    CONSTRAINT purchase_orders_receipt_after_date CHECK (expected_receipt_date IS NULL OR expected_receipt_date >= order_date)
);

CREATE INDEX purchase_orders_supplier_id_idx ON purchase_orders (supplier_id);
CREATE INDEX purchase_orders_status_idx      ON purchase_orders (status);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON purchase_orders
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE purchase_order_lines (
    id                 int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    order_id           int           NOT NULL REFERENCES purchase_orders(id) ON DELETE CASCADE,
    line_no            int           NOT NULL,
    product_id         int           REFERENCES products(id),   -- NULL = free-form line
    description        text          NOT NULL,
    quantity           numeric(19,4) NOT NULL DEFAULT 1 CHECK (quantity > 0),
    unit_cost          numeric(19,4) NOT NULL DEFAULT 0,
    expense_account_id int           REFERENCES accounts(id),
    tax_code           text          REFERENCES tax_codes(code),
    tax_rate           numeric(7,4)  NOT NULL DEFAULT 0 CHECK (tax_rate >= 0),  -- rate snapshot
    line_subtotal numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_cost, 4)) STORED,
    tax_amount    numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_cost * tax_rate / 100, 4)) STORED,
    line_total    numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_cost, 4)
                             + round(quantity * unit_cost * tax_rate / 100, 4)) STORED,
    CONSTRAINT purchase_order_lines_line_no_uq UNIQUE (order_id, line_no)
);

CREATE INDEX purchase_order_lines_order_id_idx   ON purchase_order_lines (order_id);
CREATE INDEX purchase_order_lines_product_id_idx ON purchase_order_lines (product_id);

CREATE FUNCTION trg_purchase_order_recompute() RETURNS trigger AS $$
DECLARE
    v_order  int;
    v_status text;
BEGIN
    v_order := COALESCE(NEW.order_id, OLD.order_id);
    SELECT status INTO v_status FROM purchase_orders WHERE id = v_order;
    IF v_status IS NULL OR v_status <> 'draft' THEN
        RETURN NULL;
    END IF;
    UPDATE purchase_orders po SET
        subtotal  = COALESCE(t.sub, 0),
        tax_total = COALESCE(t.tax, 0),
        total     = COALESCE(t.tot, 0)
    FROM (
        SELECT sum(line_subtotal) AS sub, sum(tax_amount) AS tax, sum(line_total) AS tot
        FROM purchase_order_lines WHERE order_id = v_order
    ) t
    WHERE po.id = v_order;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER purchase_order_lines_recompute
    AFTER INSERT OR UPDATE OR DELETE ON purchase_order_lines
    FOR EACH ROW EXECUTE FUNCTION trg_purchase_order_recompute();

-- ---------------------------------------------------------------------------
-- Fulfilment links from invoices and bills
-- ---------------------------------------------------------------------------

ALTER TABLE sales_invoice_lines
    ADD COLUMN order_line_id int REFERENCES sales_order_lines(id);
CREATE INDEX sales_invoice_lines_order_line_id_idx
    ON sales_invoice_lines (order_line_id) WHERE order_line_id IS NOT NULL;

ALTER TABLE purchase_bill_lines
    ADD COLUMN order_line_id int REFERENCES purchase_order_lines(id);
CREATE INDEX purchase_bill_lines_order_line_id_idx
    ON purchase_bill_lines (order_line_id) WHERE order_line_id IS NOT NULL;

-- An invoice line may only draw on an order line when the order is confirmed,
-- belongs to the same customer and currency, and the line's cumulative
-- invoiced quantity (across all non-void invoices) stays within what was
-- ordered. Deferred so multi-line documents are checked as a whole.
CREATE FUNCTION trg_invoice_line_order_check() RETURNS trigger AS $$
DECLARE
    v_ord_qty    numeric(19,4);
    v_ord_cust   int;
    v_ord_ccy    char(3);
    v_ord_status text;
    v_ord_no     text;
    v_inv_cust   int;
    v_inv_ccy    char(3);
    v_invoiced   numeric(19,4);
BEGIN
    IF NEW.order_line_id IS NULL THEN
        RETURN NULL;
    END IF;

    SELECT sol.quantity, so.customer_id, so.currency_code, so.status, so.order_number
        INTO v_ord_qty, v_ord_cust, v_ord_ccy, v_ord_status, v_ord_no
        FROM sales_order_lines sol
        JOIN sales_orders so ON so.id = sol.order_id
        WHERE sol.id = NEW.order_line_id;
    SELECT customer_id, currency_code INTO v_inv_cust, v_inv_ccy
        FROM sales_invoices WHERE id = NEW.invoice_id;

    IF v_ord_status <> 'open' THEN
        RAISE EXCEPTION 'sales order % is % (only open orders can be invoiced)',
            v_ord_no, v_ord_status;
    END IF;
    IF v_ord_cust <> v_inv_cust THEN
        RAISE EXCEPTION 'invoice % and sales order % belong to different customers',
            NEW.invoice_id, v_ord_no;
    END IF;
    IF v_ord_ccy <> v_inv_ccy THEN
        RAISE EXCEPTION 'invoice % (%) and sales order % (%) are in different currencies',
            NEW.invoice_id, v_inv_ccy, v_ord_no, v_ord_ccy;
    END IF;

    SELECT COALESCE(sum(l.quantity), 0) INTO v_invoiced
        FROM sales_invoice_lines l
        JOIN sales_invoices i ON i.id = l.invoice_id AND i.status <> 'void'
        WHERE l.order_line_id = NEW.order_line_id;
    IF v_invoiced > v_ord_qty THEN
        RAISE EXCEPTION 'sales order % line over-invoiced: % of % ordered',
            v_ord_no, v_invoiced, v_ord_qty;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER sales_invoice_lines_order_check
    AFTER INSERT OR UPDATE ON sales_invoice_lines
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION trg_invoice_line_order_check();

-- The purchase-side mirror: bill lines drawing on purchase-order lines.
CREATE FUNCTION trg_bill_line_order_check() RETURNS trigger AS $$
DECLARE
    v_ord_qty    numeric(19,4);
    v_ord_supp   int;
    v_ord_ccy    char(3);
    v_ord_status text;
    v_ord_no     text;
    v_bill_supp  int;
    v_bill_ccy   char(3);
    v_billed     numeric(19,4);
BEGIN
    IF NEW.order_line_id IS NULL THEN
        RETURN NULL;
    END IF;

    SELECT pol.quantity, po.supplier_id, po.currency_code, po.status, po.order_number
        INTO v_ord_qty, v_ord_supp, v_ord_ccy, v_ord_status, v_ord_no
        FROM purchase_order_lines pol
        JOIN purchase_orders po ON po.id = pol.order_id
        WHERE pol.id = NEW.order_line_id;
    SELECT supplier_id, currency_code INTO v_bill_supp, v_bill_ccy
        FROM purchase_bills WHERE id = NEW.bill_id;

    IF v_ord_status <> 'open' THEN
        RAISE EXCEPTION 'purchase order % is % (only open orders can be billed)',
            v_ord_no, v_ord_status;
    END IF;
    IF v_ord_supp <> v_bill_supp THEN
        RAISE EXCEPTION 'bill % and purchase order % belong to different suppliers',
            NEW.bill_id, v_ord_no;
    END IF;
    IF v_ord_ccy <> v_bill_ccy THEN
        RAISE EXCEPTION 'bill % (%) and purchase order % (%) are in different currencies',
            NEW.bill_id, v_bill_ccy, v_ord_no, v_ord_ccy;
    END IF;

    SELECT COALESCE(sum(l.quantity), 0) INTO v_billed
        FROM purchase_bill_lines l
        JOIN purchase_bills b ON b.id = l.bill_id AND b.status <> 'void'
        WHERE l.order_line_id = NEW.order_line_id;
    IF v_billed > v_ord_qty THEN
        RAISE EXCEPTION 'purchase order % line over-billed: % of % ordered',
            v_ord_no, v_billed, v_ord_qty;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER purchase_bill_lines_order_check
    AFTER INSERT OR UPDATE ON purchase_bill_lines
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION trg_bill_line_order_check();

-- ---------------------------------------------------------------------------
-- Reporting convenience
-- ---------------------------------------------------------------------------

-- Per-line fulfilment: what has been invoiced against the line (non-void
-- invoices) and shipped against it (issue movements are negative, so shipped
-- quantity is the negated sum). qty_to_ship is the outstanding shippable
-- quantity, which is zero for non-stock lines (services, free-form): they are
-- invoiced, never shipped, so they must not hold an order open on the shipping
-- axis.
CREATE VIEW sales_order_line_fulfilment AS
SELECT sol.id                                              AS order_line_id,
       sol.order_id,
       sol.line_no,
       sol.product_id,
       sol.description,
       sol.quantity                                        AS qty_ordered,
       COALESCE(inv.qty, 0)                                AS qty_invoiced,
       COALESCE(shp.qty, 0)                                AS qty_shipped,
       GREATEST(sol.quantity - COALESCE(inv.qty, 0), 0)    AS qty_to_invoice,
       CASE WHEN COALESCE(p.track_inventory, false)
            THEN GREATEST(sol.quantity - COALESCE(shp.qty, 0), 0)
            ELSE 0 END                                     AS qty_to_ship
FROM sales_order_lines sol
LEFT JOIN products p ON p.id = sol.product_id
LEFT JOIN (
    SELECT l.order_line_id, sum(l.quantity) AS qty
    FROM sales_invoice_lines l
    JOIN sales_invoices i ON i.id = l.invoice_id AND i.status <> 'void'
    WHERE l.order_line_id IS NOT NULL
    GROUP BY l.order_line_id
) inv ON inv.order_line_id = sol.id
LEFT JOIN (
    SELECT sm.source_id, sum(-sm.quantity) AS qty
    FROM stock_movements sm
    WHERE sm.source_type = 'sales_order_line'
    GROUP BY sm.source_id
) shp ON shp.source_id = sol.id;

-- Header rollup on each axis: 'none' until something happens, 'invoiced'
-- /'shipped' once nothing outstanding remains, else 'partial'. Summing the
-- per-line remainders (which are already zero for lines that axis does not
-- apply to) keeps services from blocking the shipping status; COALESCE handles
-- an order with no lines.
CREATE VIEW sales_order_fulfilment AS
SELECT so.id                       AS order_id,
       so.order_number,
       so.customer_id,
       so.order_date,
       so.expected_ship_date,
       so.currency_code,
       so.status,
       so.total,
       CASE
           WHEN COALESCE(sum(f.qty_invoiced), 0) = 0   THEN 'none'
           WHEN COALESCE(sum(f.qty_to_invoice), 0) = 0 THEN 'invoiced'
           ELSE 'partial'
       END AS invoiced_status,
       CASE
           WHEN COALESCE(sum(f.qty_shipped), 0) = 0 THEN 'none'
           WHEN COALESCE(sum(f.qty_to_ship), 0) = 0 THEN 'shipped'
           ELSE 'partial'
       END AS shipped_status
FROM sales_orders so
LEFT JOIN sales_order_line_fulfilment f ON f.order_id = so.id
GROUP BY so.id;

CREATE VIEW purchase_order_line_fulfilment AS
SELECT pol.id                                              AS order_line_id,
       pol.order_id,
       pol.line_no,
       pol.product_id,
       pol.description,
       pol.quantity                                        AS qty_ordered,
       COALESCE(bil.qty, 0)                                AS qty_billed,
       COALESCE(rcv.qty, 0)                                AS qty_received,
       GREATEST(pol.quantity - COALESCE(bil.qty, 0), 0)    AS qty_to_bill,
       CASE WHEN COALESCE(p.track_inventory, false)
            THEN GREATEST(pol.quantity - COALESCE(rcv.qty, 0), 0)
            ELSE 0 END                                     AS qty_to_receive
FROM purchase_order_lines pol
LEFT JOIN products p ON p.id = pol.product_id
LEFT JOIN (
    SELECT l.order_line_id, sum(l.quantity) AS qty
    FROM purchase_bill_lines l
    JOIN purchase_bills b ON b.id = l.bill_id AND b.status <> 'void'
    WHERE l.order_line_id IS NOT NULL
    GROUP BY l.order_line_id
) bil ON bil.order_line_id = pol.id
LEFT JOIN (
    SELECT sm.source_id, sum(sm.quantity) AS qty
    FROM stock_movements sm
    WHERE sm.source_type = 'purchase_order_line'
    GROUP BY sm.source_id
) rcv ON rcv.source_id = pol.id;

CREATE VIEW purchase_order_fulfilment AS
SELECT po.id                       AS order_id,
       po.order_number,
       po.supplier_id,
       po.order_date,
       po.expected_receipt_date,
       po.currency_code,
       po.status,
       po.total,
       CASE
           WHEN COALESCE(sum(f.qty_billed), 0) = 0   THEN 'none'
           WHEN COALESCE(sum(f.qty_to_bill), 0) = 0  THEN 'billed'
           ELSE 'partial'
       END AS billed_status,
       CASE
           WHEN COALESCE(sum(f.qty_received), 0) = 0   THEN 'none'
           WHEN COALESCE(sum(f.qty_to_receive), 0) = 0 THEN 'received'
           ELSE 'partial'
       END AS received_status
FROM purchase_orders po
LEFT JOIN purchase_order_line_fulfilment f ON f.order_id = po.id
GROUP BY po.id;

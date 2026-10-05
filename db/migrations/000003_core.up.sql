-- 000003_core: the core entities most business-management modules reference.
-- Synthetic primary keys are auto-incrementing integers per the project rules.

-- Application users (people who log in to the system).
CREATE TABLE users (
    id            int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email         citext      NOT NULL UNIQUE,
    full_name     text        NOT NULL,
    password_hash text        NOT NULL,
    is_active     boolean     NOT NULL DEFAULT true,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Organizations: any business entity the system tracks. The same table covers
-- our own company, customers, suppliers, and prospects; the relationship roles
-- (customer/supplier/etc.) are layered on by the sales and purchasing modules
-- rather than baked in here, so an organization can be more than one at once.
CREATE TABLE organizations (
    id               int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name             text        NOT NULL,                 -- common/trading name
    legal_name       text,                                 -- registered legal name, if different
    tax_id           text,                                 -- VAT/EIN/etc., format varies by country
    country_code     char(2)     REFERENCES countries(code),
    default_currency char(3)     REFERENCES currencies(code),
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX organizations_country_code_idx ON organizations (country_code);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON organizations
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Postal addresses. An organization may have several (billing, shipping, ...).
CREATE TABLE addresses (
    id              int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organization_id int         NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    label           text,                                  -- e.g. 'Head office', 'Warehouse'
    line1           text        NOT NULL,
    line2           text,
    city            text        NOT NULL,
    region          text,                                  -- state/province/county
    postal_code     text,
    country_code    char(2)     NOT NULL REFERENCES countries(code),
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX addresses_organization_id_idx ON addresses (organization_id);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON addresses
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- People associated with an organization (the human point of contact).
CREATE TABLE contacts (
    id              int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organization_id int         NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    first_name      text        NOT NULL,
    last_name       text        NOT NULL,
    title           text,                                  -- job title
    email           citext,
    phone           text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX contacts_organization_id_idx ON contacts (organization_id);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON contacts
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

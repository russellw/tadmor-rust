-- 000002_reference: globally-shared lookup data keyed on stable ISO codes.
-- These use natural keys per the project's schema-design rules: the codes are
-- internationally standardised, stable, and meaningful in the data itself.

-- ISO 3166-1 countries, keyed on the alpha-2 code.
CREATE TABLE countries (
    code         char(2)     PRIMARY KEY,          -- ISO 3166-1 alpha-2, e.g. 'US'
    alpha3       char(3)     NOT NULL UNIQUE,       -- ISO 3166-1 alpha-3, e.g. 'USA'
    numeric_code char(3)     NOT NULL UNIQUE,       -- ISO 3166-1 numeric, e.g. '840'
    name         text        NOT NULL,
    CONSTRAINT countries_code_upper  CHECK (code = upper(code)),
    CONSTRAINT countries_alpha3_upper CHECK (alpha3 = upper(alpha3))
);

-- ISO 4217 currencies, keyed on the alphabetic code.
CREATE TABLE currencies (
    code       char(3)  PRIMARY KEY,               -- ISO 4217 alphabetic, e.g. 'USD'
    numeric_code char(3) NOT NULL UNIQUE,          -- ISO 4217 numeric, e.g. '840'
    name       text     NOT NULL,
    symbol     text,                               -- display symbol, e.g. '$' (not unique)
    minor_unit smallint NOT NULL DEFAULT 2,        -- decimal places, e.g. 2 for USD, 0 for JPY
    CONSTRAINT currencies_code_upper  CHECK (code = upper(code)),
    CONSTRAINT currencies_minor_unit_range CHECK (minor_unit BETWEEN 0 AND 4)
);

-- A starter set of common reference data. The full ISO lists can be loaded by
-- the ancillary seed script (db/seed/, `make seed-iso`); this keeps local
-- development and tests working out of the box. ON CONFLICT DO NOTHING makes
-- re-running harmless.
INSERT INTO countries (code, alpha3, numeric_code, name) VALUES
    ('US', 'USA', '840', 'United States of America'),
    ('GB', 'GBR', '826', 'United Kingdom'),
    ('IE', 'IRL', '372', 'Ireland'),
    ('CA', 'CAN', '124', 'Canada'),
    ('AU', 'AUS', '036', 'Australia'),
    ('DE', 'DEU', '276', 'Germany'),
    ('FR', 'FRA', '250', 'France'),
    ('JP', 'JPN', '392', 'Japan')
ON CONFLICT (code) DO NOTHING;

INSERT INTO currencies (code, numeric_code, name, symbol, minor_unit) VALUES
    ('USD', '840', 'US Dollar',        '$',  2),
    ('GBP', '826', 'Pound Sterling',   '£',  2),
    ('EUR', '978', 'Euro',             '€',  2),
    ('CAD', '124', 'Canadian Dollar',  '$',  2),
    ('AUD', '036', 'Australian Dollar','$',  2),
    ('JPY', '392', 'Yen',              '¥',  0)
ON CONFLICT (code) DO NOTHING;

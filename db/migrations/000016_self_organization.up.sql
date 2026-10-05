-- 000016_self_organization: mark which organization is our own company.
--
-- The organizations table has always been documented as covering "our own
-- company" as well as customers and suppliers, but nothing identified which
-- row that is. Printable documents (invoice PDFs) need the issuer's name,
-- tax id, and address, so record it explicitly. A partial unique index
-- enforces at most one self organization; having none is allowed (the PDF
-- simply omits the seller block until it is configured).

ALTER TABLE organizations
    ADD COLUMN is_self boolean NOT NULL DEFAULT false;

CREATE UNIQUE INDEX organizations_is_self_uq ON organizations ((true)) WHERE is_self;

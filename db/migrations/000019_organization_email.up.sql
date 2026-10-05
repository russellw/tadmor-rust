-- 000019_organization_email: record a contact email on organizations.
--
-- Printable documents can be emailed to their counterparty as a PDF
-- attachment (POST /api/<collection>/{id}/email). Until now the recipient had
-- to be typed on every send because organizations carried no address; this
-- column lets a blank request resolve the counterparty's email from the
-- customer or supplier organization. It is optional (NULL when unknown), and
-- the send still accepts an explicit "to" that overrides it.

ALTER TABLE organizations
    ADD COLUMN email text;

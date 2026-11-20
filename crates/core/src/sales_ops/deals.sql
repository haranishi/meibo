CREATE TABLE IF NOT EXISTS crm_opportunity_ownership(
    opportunity_id INTEGER PRIMARY KEY REFERENCES crm_opportunities(id),
    owner_override INTEGER NOT NULL CHECK (owner_override IN (0, 1)),
    owner_id INTEGER CHECK (owner_id IS NULL OR (typeof(owner_id) = 'integer' AND owner_id > 0)),
    probability INTEGER CHECK (probability IS NULL OR (typeof(probability) = 'integer' AND probability BETWEEN 0 AND 100)),
    CHECK (owner_override = 1 OR owner_id IS NULL)
);
CREATE TRIGGER IF NOT EXISTS crm_opportunity_forecast_erase AFTER UPDATE OF erased_at ON crm_opportunities
WHEN OLD.erased_at IS NULL AND NEW.erased_at IS NOT NULL BEGIN
    DELETE FROM crm_opportunity_ownership WHERE opportunity_id = NEW.id;
END;

CREATE TABLE IF NOT EXISTS crm_sales_targets(
    month TEXT NOT NULL CHECK (
        month GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]'
        AND substr(month, 1, 4) <> '0000' AND substr(month, 6, 2) BETWEEN '01' AND '12'
    ),
    owner_id INTEGER NOT NULL CHECK (typeof(owner_id) = 'integer' AND owner_id > 0),
    amount_jpy INTEGER NOT NULL CHECK (typeof(amount_jpy) = 'integer' AND amount_jpy BETWEEN 0 AND 9007199254740991),
    PRIMARY KEY (month, owner_id)
);

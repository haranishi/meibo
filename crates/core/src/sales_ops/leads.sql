CREATE TABLE IF NOT EXISTS crm_lead_form (
    id INTEGER PRIMARY KEY CHECK(id=1),
    public_key TEXT NOT NULL UNIQUE CHECK(length(public_key)=32 AND public_key NOT GLOB '*[^a-f0-9]*')
);
CREATE TABLE IF NOT EXISTS crm_leads (
    id INTEGER PRIMARY KEY CHECK(id>0),
    submission_key TEXT NOT NULL UNIQUE CHECK(length(submission_key)=32 AND submission_key NOT GLOB '*[^a-f0-9]*'),
    created_at INTEGER NOT NULL CHECK(created_at>=0),
    company_name TEXT,
    contact_name TEXT,
    email TEXT,
    message TEXT,
    owner_id INTEGER CHECK(owner_id IS NULL OR owner_id>0),
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision>0),
    state INTEGER NOT NULL DEFAULT 0 CHECK(state IN (0,1,2)),
    company_id INTEGER REFERENCES companies(id),
    contact_id INTEGER REFERENCES contacts(id),
    opportunity_id INTEGER REFERENCES crm_opportunities(id),
    CHECK(
        (state=0 AND company_name IS NOT NULL AND length(company_name) BETWEEN 1 AND 200
                 AND contact_name IS NOT NULL AND length(contact_name) BETWEEN 1 AND 100
                 AND email IS NOT NULL AND length(email) BETWEEN 3 AND 254
                 AND (message IS NULL OR length(message)<=1000)
                 AND company_id IS NULL AND contact_id IS NULL AND opportunity_id IS NULL)
        OR (state IN (1,2) AND company_name IS NULL AND contact_name IS NULL AND email IS NULL AND message IS NULL
            AND ((state=1 AND company_id IS NOT NULL AND contact_id IS NOT NULL)
                 OR (state=2 AND company_id IS NULL AND contact_id IS NULL AND opportunity_id IS NULL)))
    )
);
CREATE INDEX IF NOT EXISTS ix_lead_received ON crm_leads(created_at);
CREATE INDEX IF NOT EXISTS ix_lead_state_id ON crm_leads(state,id);

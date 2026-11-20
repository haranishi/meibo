CREATE TABLE IF NOT EXISTS crm_call_followups (
    activity_id INTEGER PRIMARY KEY REFERENCES activities(id),
    work_id INTEGER NOT NULL UNIQUE REFERENCES crm_work_items(id),
    owner_id INTEGER CHECK(owner_id IS NULL OR owner_id>0),
    request_key TEXT NOT NULL UNIQUE CHECK(length(request_key) BETWEEN 16 AND 80),
    fingerprint TEXT NOT NULL CHECK(length(fingerprint)=64),
    scheduled_by INTEGER REFERENCES activities(id)
);
CREATE INDEX IF NOT EXISTS crm_followup_owner ON crm_call_followups(owner_id,work_id);
CREATE TRIGGER IF NOT EXISTS crm_cancel_followup AFTER INSERT ON activities WHEN NEW.kind=9 BEGIN
    UPDATE crm_work_items SET erased_at=NEW.at, updated_at=NEW.at
    WHERE id=(SELECT work_id FROM crm_call_followups WHERE activity_id=NEW.reverses_id) AND erased_at IS NULL;
END;

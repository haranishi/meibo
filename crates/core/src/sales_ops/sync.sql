CREATE TABLE IF NOT EXISTS crm_sync_links(
  source_key TEXT PRIMARY KEY NOT NULL CHECK(length(source_key)=64),
  activity_id INTEGER UNIQUE REFERENCES activities(id),
  work_item_id INTEGER UNIQUE REFERENCES crm_work_items(id),
  CHECK((activity_id IS NULL) <> (work_item_id IS NULL))
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS crm_sync_contact_email ON contacts(lower(trim(email))) WHERE erased_at IS NULL;
CREATE TRIGGER IF NOT EXISTS crm_sync_erase_contact_mail AFTER UPDATE OF erased_at ON contacts
WHEN OLD.erased_at IS NULL AND NEW.erased_at IS NOT NULL BEGIN
  UPDATE activities SET note=NULL,note_erased_at=NEW.erased_at
  WHERE contact_id=NEW.id AND note IS NOT NULL AND id IN(SELECT activity_id FROM crm_sync_links WHERE activity_id IS NOT NULL);
END;
CREATE TRIGGER IF NOT EXISTS crm_sync_erase_company_mail AFTER UPDATE OF erased_at ON companies
WHEN OLD.erased_at IS NULL AND NEW.erased_at IS NOT NULL BEGIN
  UPDATE activities SET note=NULL,note_erased_at=NEW.erased_at
  WHERE company_id=NEW.id AND note IS NOT NULL AND id IN(SELECT activity_id FROM crm_sync_links WHERE activity_id IS NOT NULL);
END;

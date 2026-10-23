CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE companies(
  id INTEGER PRIMARY KEY,
  corporate_number TEXT,
  name TEXT NOT NULL,
  name_key TEXT NOT NULL,
  prefecture_code INTEGER NOT NULL,
  municipality_code TEXT,
  industry_code INTEGER,
  status INTEGER NOT NULL DEFAULT 0,
  phone TEXT,
  website TEXT,
  has_phone INTEGER GENERATED ALWAYS AS (phone IS NOT NULL) STORED,
  owner_id INTEGER,
  last_activity_at INTEGER NOT NULL DEFAULT 0,
  next_action_at INTEGER,
  base_status INTEGER NOT NULL DEFAULT 0,
  is_sole_proprietor INTEGER NOT NULL DEFAULT 0,
  source_id TEXT,
  source_as_of TEXT,
  created_at INTEGER NOT NULL,
  erased_at INTEGER,
  updated_at INTEGER NOT NULL);
CREATE UNIQUE INDEX u1_corporate_number ON companies(corporate_number) WHERE corporate_number IS NOT NULL;
CREATE INDEX k1 ON companies(prefecture_code, industry_code, status, has_phone, id);
CREATE INDEX k2 ON companies(prefecture_code, status, has_phone, id);
CREATE INDEX k3 ON companies(municipality_code, industry_code, status, has_phone, id);
CREATE INDEX k4 ON companies(municipality_code, status, has_phone, id);
CREATE INDEX k5 ON companies(industry_code, status, has_phone, id);
CREATE INDEX k6 ON companies(status, has_phone, id);
CREATE INDEX k7 ON companies(has_phone, status, id);
CREATE INDEX k8 ON companies(owner_id, next_action_at, status, id) WHERE owner_id IS NOT NULL;
CREATE INDEX k9 ON companies(next_action_at, status, id) WHERE next_action_at IS NOT NULL;
CREATE INDEX k10 ON companies(last_activity_at, status, id);
CREATE TABLE contacts(
  id INTEGER PRIMARY KEY,
  company_id INTEGER NOT NULL REFERENCES companies(id),
  name TEXT, phone TEXT, email TEXT,
  created_at INTEGER NOT NULL, erased_at INTEGER);
CREATE INDEX ix_contacts_company ON contacts(company_id, id);
CREATE TABLE activities(
  id INTEGER PRIMARY KEY,
  company_id INTEGER NOT NULL REFERENCES companies(id),
  contact_id INTEGER REFERENCES contacts(id),
  kind INTEGER NOT NULL,
  at INTEGER NOT NULL,
  note TEXT,
  status_after INTEGER,
  next_action_kind INTEGER NOT NULL DEFAULT 0,
  next_action_at INTEGER,
  reverses_id INTEGER REFERENCES activities(id),
  note_erased_at INTEGER);
CREATE INDEX ix_activities_company ON activities(company_id, at DESC, id DESC);
CREATE UNIQUE INDEX u2_activities_reverses ON activities(reverses_id) WHERE reverses_id IS NOT NULL;
CREATE TABLE lists(id INTEGER PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE list_members(list_id INTEGER NOT NULL REFERENCES lists(id) ON DELETE CASCADE, company_id INTEGER NOT NULL,
  PRIMARY KEY(list_id, company_id)) WITHOUT ROWID;
CREATE TABLE counts(status INTEGER NOT NULL, has_phone INTEGER NOT NULL, n INTEGER NOT NULL, PRIMARY KEY(status, has_phone)) WITHOUT ROWID;
CREATE TABLE erase_gate(x INTEGER);
CREATE VIRTUAL TABLE name_fts USING fts5(name_key, content='companies', content_rowid='id', tokenize='trigram case_sensitive 1');
CREATE TRIGGER activities_no_delete BEFORE DELETE ON activities BEGIN SELECT RAISE(ABORT,'activities are append-only'); END;
CREATE TRIGGER activities_guard BEFORE UPDATE ON activities
WHEN NOT EXISTS (SELECT 1 FROM erase_gate)
  OR NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.contact_id IS NOT OLD.contact_id
  OR NEW.kind IS NOT OLD.kind OR NEW.at IS NOT OLD.at OR NEW.status_after IS NOT OLD.status_after
  OR NEW.next_action_kind IS NOT OLD.next_action_kind OR NEW.next_action_at IS NOT OLD.next_action_at
  OR NEW.reverses_id IS NOT OLD.reverses_id
  OR NEW.note IS NOT NULL
  OR NEW.note_erased_at IS NULL
  OR OLD.note_erased_at IS NOT NULL
BEGIN SELECT RAISE(ABORT,'activities are append-only'); END;

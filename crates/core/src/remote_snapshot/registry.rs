/// One immutable, trusted CRM table definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrmTable {
    name: &'static str,
    columns: &'static [&'static str],
    key_columns: &'static [&'static str],
}

impl CrmTable {
    /// Trusted table name, never obtained from a snapshot document.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }
    /// Ordered physical columns, excluding generated values.
    #[must_use]
    pub const fn columns(&self) -> &'static [&'static str] {
        self.columns
    }
    /// Ordered primary-key columns used by later keyset readers.
    #[must_use]
    pub const fn key_columns(&self) -> &'static [&'static str] {
        self.key_columns
    }
}

const TABLES: &[CrmTable] = &[
    CrmTable {
        name: "meta",
        columns: &["key", "value"],
        key_columns: &["key"],
    },
    CrmTable {
        name: "companies",
        columns: &[
            "id",
            "corporate_number",
            "name",
            "name_key",
            "prefecture_code",
            "municipality_code",
            "industry_code",
            "status",
            "phone",
            "website",
            "owner_id",
            "last_activity_at",
            "next_action_at",
            "base_status",
            "is_sole_proprietor",
            "source_id",
            "source_as_of",
            "created_at",
            "erased_at",
            "updated_at",
        ],
        key_columns: &["id"],
    },
    CrmTable {
        name: "contacts",
        columns: &[
            "id",
            "company_id",
            "name",
            "phone",
            "email",
            "created_at",
            "erased_at",
        ],
        key_columns: &["id"],
    },
    CrmTable {
        name: "activities",
        columns: &[
            "id",
            "company_id",
            "contact_id",
            "kind",
            "at",
            "note",
            "status_after",
            "next_action_kind",
            "next_action_at",
            "reverses_id",
            "note_erased_at",
        ],
        key_columns: &["id"],
    },
    CrmTable {
        name: "lists",
        columns: &["id", "name", "created_at"],
        key_columns: &["id"],
    },
    CrmTable {
        name: "list_members",
        columns: &["list_id", "company_id"],
        key_columns: &["list_id", "company_id"],
    },
    CrmTable {
        name: "counts",
        columns: &["status", "has_phone", "n"],
        key_columns: &["status", "has_phone"],
    },
    CrmTable {
        name: "crm_work_items",
        columns: &[
            "id",
            "company_id",
            "contact_id",
            "kind",
            "subject",
            "due_at",
            "start_at",
            "end_at",
            "completed",
            "created_at",
            "updated_at",
            "erased_at",
        ],
        key_columns: &["id"],
    },
    CrmTable {
        name: "crm_opportunities",
        columns: &[
            "id",
            "company_id",
            "contact_id",
            "name",
            "amount_jpy",
            "stage",
            "close_date",
            "created_at",
            "updated_at",
            "erased_at",
        ],
        key_columns: &["id"],
    },
    CrmTable {
        name: "custom_field_definitions",
        columns: &["id", "name", "kind", "is_indexed"],
        key_columns: &["id"],
    },
    CrmTable {
        name: "custom_field_values",
        columns: &[
            "field_id",
            "company_id",
            "kind",
            "is_indexed",
            "text_value",
            "number_value",
            "boolean_value",
        ],
        key_columns: &["field_id", "company_id"],
    },
    CrmTable {
        name: "erased_corporate_numbers",
        columns: &["number_hash"],
        key_columns: &["number_hash"],
    },
];

/// Return the closed CRM profile, excluding control data and derived tables.
#[must_use]
pub fn crm_tables() -> &'static [CrmTable] {
    TABLES
}

/// Resolve an exact table name within the closed CRM profile.
#[must_use]
pub fn crm_table(name: &str) -> Option<&'static CrmTable> {
    crm_tables().iter().find(|table| table.name == name)
}

table! {
    datastore (hash) {
        hash -> Bytea,
        data -> Bytea,
    }
}

table! {
    events (sequence) {
        sequence -> Int8,
        version -> Int4,
        #[sql_name = "type"]
        type_ -> VarChar,
        data -> Jsonb,
        timestamp -> Timestamptz,
    }
}

allow_tables_to_appear_in_same_query!(datastore, events,);

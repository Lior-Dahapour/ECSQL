# ECSQL: Design Document

A client/server, transactional database (like PostgreSQL) built from scratch around the Entity Component System model, written in Rust. Instead of tables that own rows, ECSQL has global entity IDs and typed components. Entities with the same set of components are stored together (an archetype), whole rows per page, so loading an entity is one page read and combining components in a query is positional, not a join. Components that toggle often can be stored separately so toggling them never moves the entity.

> Status: very early prototype. Nothing here is closed: the document is updated as each step is designed and built. Items marked **[OPEN]** are undecided, **[DECIDED]** items were settled during implementation (see section 12, the decision log), and **[PLANNED]** items are agreed but not built yet.

---

## 1. Goals and non-goals

### Goals
- Entity-first data model: an ID is allocated first, and components are attached to it afterwards.
- **[DECIDED] Logical, NULL-free model.** An entity has exactly the components it has. No wide tables with mostly-NULL columns, no table per class, no join tables: a warrior is `(Warrior, Player, Stats)` and a mage is `(Mage, Player, MagicStats)`. Queries should read like the application's own logic ("players with an inventory who are warriors and not banned"), and storage should follow the same shape.
- No joins on ID. Multi-component queries walk the archetypes that have the component set; inside a table, the same row is the same entity in every column.
- Components are defined at runtime (DDL), not as compile-time Rust types.
- Columnar, cache-friendly storage, in memory and on disk.
- Cheap, first-class "add/remove a component" operation.
- Durable, transactional, crash-safe.
- Client/server, like PostgreSQL: the database runs as its own process and clients connect over the network. Not an embedded library.
- The storage core (`ecsql-core`) is `no_std` + `alloc`, with no OS dependency. Disk, networking, and threads are layered on top.
- Proving ground: persistence for a game server (an MMO backend). Not limited to games: any domain whose objects gain and lose properties over time.

### Workload: transactional and dynamic **[DECIDED]**
ECSQL is a transactional (OLTP) database first. Reads and writes mostly touch a few entities: load one, save one, change some of its components. Data is dynamic: entities gain and lose components all the time, and that must be as cheap and simple as writing it in your own game code. No schema migration, no join tables, no moving data around. Analytics (OLAP: scanning many entities to compute summaries) must work but does not need to be the fastest.

### Workload: storage first **[DECIDED]**
ECSQL is the database a game server saves to and loads from, not the live game world. The game server keeps live state in its own memory and talks to ECSQL for logins (point lookups), saves (batched writes in one transaction), inventories, and queries. So:
- The data on disk is the truth and can be much larger than RAM. Memory is a cache (the buffer pool).
- Per-tick update rates (for example 120 Hz) are not the target workload.
- What matters most: crash safety, fast point lookups, compact pages, and scans that read only the columns they need.

### Non-goals (at least initially)
- Full SQL compatibility. ECSQL is SQL-flavoured, not SQL.
- Distributed operation or replication.
- Ad-hoc many-to-many joins as a primary feature.
- Beating DuckDB at general analytics. The pitch is the entity-and-component model, plus speed on the workloads it fits.

---

## 2. Data model

| Concept | Meaning |
|---|---|
| **Database** | A named container, for example `MYGAMEDB`. Owns one world. |
| **Entity** | A global ID: `(index: u32, generation: u32)`. Owned by no table. |
| **Component type** | A named, typed schema, defined at runtime. Has a numeric `ComponentId`. |
| **Component value** | A fixed-schema record attached to an entity. At most one value per component type per entity. |
| **Archetype** | The unique *set* of (non-sparse) component types an entity has. Order does not matter. |
| **Table** | Storage for all entities of one archetype: PAX pages, each holding whole rows with every component's column contiguous inside the page, plus an entity-ID column. |
| **Row** | The position within a table. The same row index across all columns is the same entity. |
| **Sparse component** | Opt-in (`SPARSE`) for components that toggle often: stored outside archetype tables, by entity index, with a presence bit. Toggling it never moves the entity. |

### Component schemas
A component is a record of typed fields. A **tag** is a component with zero fields.

```sql
CREATE COMPONENT Position (x f32, y f32);
CREATE COMPONENT Health   (current i32, max i32);
CREATE COMPONENT Name     (value text);
CREATE COMPONENT Dead;                          -- tag, no fields
CREATE COMPONENT Stunned  SPARSE;               -- stored in a sparse side table
CREATE COMPONENT Parent   (id entity);          -- relation: holds another entity's ID
```

Initial field types: `i8..i64`, `u8..u64`, `f32`, `f64`, `bool`, `text`, `bytes`, `entity`. **[OPEN]** arrays, optional fields, nested records.

### Relations keep entities small **[DECIDED]**
Things an entity owns are their own entities, linked by an `entity` field, not nested inside it. A player's items are item entities; the player row holds only the player's own components. So rows stay small (well under a page), and moving the player to another archetype never touches its items.

**[DECIDED] The link lives on the owned entity.** An item has `ItemOwner(owner entity)`; the player holds no list. Listing a player's items is `SELECT Item WHERE ItemOwner.owner = e`, made fast by an index on `ItemOwner.owner`. Adding or removing an item touches only the item, rows stay fixed-size, and the owner never changes. (A list inside the owner would be variable-length, need an overflow store, and rewrite the owner on every change.) Flecs models relations the same way (`ChildOf` pairs on the child).

**[OPEN]** What happens to owned entities when the owner is despawned: nothing (dangling link, detectable through the generation), cascade delete, or an error, like a foreign key's `ON DELETE`.

- **`text` [PLANNED]:** a fixed 16-byte, 8-aligned "German string" (Umbra, `ref/books/umbra-german-strings.pdf`): `len: u32` plus 12 inline bytes when short, or `len: u32`, a 4-byte prefix and an 8-byte **offset** into a string-heap page when long. It is an offset, never a raw pointer, so the value means the same thing on disk. Equality on short strings is two `u64` compares.

### Component row layout **[DECIDED]**
- One column per component, storing whole component values (AoS within the column). In ECS the logic lives in the component, not in single fields: reads and updates normally take or replace a whole component ("update `BaseStats` for e"), not one field. So a component update overwrites one contiguous slot in one page, and the WAL logs the whole value.
- A row is laid out like a C struct, but fields are **stable-sorted by alignment, largest first**, which removes interior padding. The row size is rounded up to the largest alignment (tail padding), so every row in a column stays aligned.
- `fields` stays in declaration order: that order is the field's identity and is append-only (PostgreSQL `attnum` lesson). Only the byte `offset` reflects the physical order.
- **[PLANNED]** For portability on disk, `FieldType` alignments become fixed constants (not `align_of`, which is 4 for `u64` on 32-bit x86), and the on-disk byte order is little-endian.

### Entity IDs
- **[DECIDED]** An `Entity` behaves as a packed `u64`: `generation << 32 | index`. `generation` is a `NonZeroU32` (starts at 1), so `Option<Entity>` is still 8 bytes.
- `index` points into the **entity metadata array**. Each slot stores `{ generation, archetype_id, row }`, or a free-list link when unused. Looking up an entity is an array index plus a generation compare: no hashing.
- `generation` increments when a slot is freed, so stale IDs are detected rather than silently matching a reused slot.
- **[OPEN]** Which free index to reuse. Lowest-first keeps sparse stores dense.
- IDs can be allocated in blocks, with no storage touched, so a client can reserve IDs before knowing what the entity will be.
- Everything that refers to an entity (indexes, WAL records, relations) stores the `Entity`, never a physical `(table, row)`. Rows move on `swap_remove` and archetype moves; only the metadata slot changes.

---

## 3. Storage architecture

### 3.1 Core data structures **[DECIDED: archetype tables in PAX pages, plus sparse stores]**

```
World
 ├─ components:  Vec<ComponentInfo>              // ComponentId -> schema, layout, storage kind
 ├─ entities:    EntityMeta[]                    // index -> { generation, archetype, row }
 ├─ archetypes:  Vec<Archetype>
 ├─ arch_index:  HashMap<SortedComponentSet, ArchetypeId>
 └─ sparse:      Vec<SparseStore>                // one per SPARSE component

Archetype
 ├─ id, component_set (sorted, table components only)
 ├─ table: TableId
 └─ edges: { add: ArchetypeId, remove: ArchetypeId } per component   // cache

Table (one per archetype)                        // a list of PAX pages
 └─ pages: each holds N whole entities of this archetype (N fixed per archetype)
           [header | entity IDs × N | component A × N | component B × N | ...]
           row r lives in page r / N, slot r % N

SparseStore (opt-in per component: CREATE COMPONENT ... SPARSE)
 ├─ presence: 1 bit per entity index, in small presence pages (8 KiB covers 65,536 entities)
 └─ data:     paged array addressed by entity index (page e.index >> k, slot e.index & mask),
              or a B-tree keyed by entity index for very rare components
```

**Why archetype tables (PAX pages) by default.** ECSQL's main work is loading and saving whole entities. A PAX page (Ailamaki et al., "Weaving relations for cache performance", VLDB 2001; reference [6] in `ref/books/column-stores-vs-row-stores.pdf`) keeps whole rows of one archetype in a page, but stores each component's column contiguously inside the page. Unity's ECS stores archetypes the same way, in 16 KiB chunks. So:
- Loading an entity reads one page, as a row store does (PostgreSQL `heap_fetch` does a single `ReadBuffer`, `heapam.c`), instead of one page per component.
- Inside the page, columns are contiguous, so scans still get column-style cache behaviour.
- A move after adding or removing a component copies one row: source page, the source's last page (to fill the hole), the destination's last page, and two metadata entries. Moves are occasional: entities are mostly created with their components and changed here and there.

Costs per operation for a 30-component entity:

| Operation | Archetype + PAX pages (chosen) | Entity-indexed stores (rejected as default) |
|---|---|---|
| GET e (login) | **1 page** (more if the row is large) | 30 pages |
| SPAWN | 1 page (the archetype's last page) | k pages |
| UPDATE a component | 1 page | 1 page |
| Add / remove a component | about 5 pages (move one row) | 1 page + metadata |
| DESPAWN | 2 pages + 2 metadata (swap-remove) | k pages + metadata |
| Scan one component | whole pages of matching archetypes; every row matches | dense only with allocation pools |

Bevy made the same choice: `StorageType::Table` is the default ("fast and cache-friendly iteration, but slower addition and removal of components") and `StorageType::SparseSet` is opt-in ("fast addition and removal of components, but slower iteration"), `ref/src/bevy/crates/bevy_ecs/src/component/mod.rs`.

**SPARSE components.** Components that are toggled often (`Banned`, `Online`, short-lived states) are declared `SPARSE`. They are not part of the archetype's table, so adding or removing them never moves the entity: it sets or clears a presence bit. Archetypes that differ only by sparse components share one table. Sparse tags have no data pages at all, so they are also the cheapest filters (section 3.2).

**[OPEN] Rows that do not fit a page.** A PAX page needs at least one whole row. Options: a maximum row size per archetype, with large components forced to `SPARSE` or to an overflow store; or splitting very wide archetypes into column groups across pages. Decide with the page format.

**[OPEN] Archetype explosion.** Many rare component combinations create many small tables, each with a partly filled last page. `SPARSE` for rare or optional components is the main defence, as in Bevy and Flecs.

**Table (built in `ecsql-core/src/table`, in memory so far):**
- Sorted parallel arrays instead of a `HashMap<ComponentId, Column>`. An archetype rarely has more than about 20 components, so a binary search is as fast as hashing, queries resolve column indices once per table anyway, and sorted IDs let a move between tables be a merge walk with no lookups. (Bevy uses a sparse set here; its sparse array is as long as the largest `ComponentId`, per table, which does not fit thousands of runtime-defined components.)
- Invariant: every column has exactly `entities.len()` rows.
- Operations: `push(entity, rows)`, `reserve(n)`, `swap_remove(row) -> Option<Entity>` (returns the entity moved into `row`, whose metadata the World must update), and `move_row_to(row, dst, added) -> (dst_row, moved)`.
- `move_row_to` walks both sorted ID lists: shared components move across, components only in the source are dropped, and components only in the destination are filled from `added`, in the destination's ID order. The Table does **not** validate `added`: that is the caller's contract, guaranteed by the World from the archetype edge. `Column::push` still asserts sizes, so a bug cannot overflow memory.
- On disk, a PAX page is this same structure with a fixed capacity N, so these operations become the page-level operations.
- **[PLANNED]** A row index newtype `TableRow(u32)` with a niche, as in Bevy (`TableRow(NonMaxU32)`), to keep entity metadata records small.
- **[PLANNED]** Cache a "move plan" per archetype edge (column index pairs, dropped columns, filled columns) so a move skips the merge walk.

Rules:
- `arch_index` maps the **sorted** component set to an archetype ID. Spawn or insert looks the set up, reusing the archetype or creating it (with its table).
- A structural change to a table component moves the entity's row to the target archetype's table, swap-removes the old row, and fixes the metadata of the entity that was moved into the hole.
- The edges cache makes "A plus component X" an O(1) hop to archetype B.
- Component values are runtime-typed bytes with a per-component layout (size, alignment, field offsets). **No generics over component types in the hot path.**

### 3.2 Query execution

A query is resolved to:
1. `with`: the component set that must be present.
2. `without`: the component set that must be absent.
3. A list of matching archetype IDs, cached per query and **updated incrementally** when a new archetype is created, never by re-scanning every archetype each time.

Execution:
```
for archetype in matching_archetypes:           // table components decide the archetypes
    for page in archetype.table.pages:
        for slot in 0..page.row_count:
            if a sparse `with`/`without` is present: check its presence bit for this entity
            read the slot from each requested column inside the page
```
`WITHOUT` on a table component excludes whole archetypes with no per-row check. Filters (`WHERE Health.current < 10`) apply per row on the column data. Aggregates (`AVG(Health.current)`) scan one column per page. Both should be vectorized over the page's column slices.

**Tags as filters [DECIDED].** Tags (components with no fields, like `Warrior` or `Banned`) cost almost nothing to filter by. A table tag (a class marker like `Warrior`, set at spawn and rarely changed) is part of the archetype, so it selects archetypes and no row is checked. A sparse tag (a status like `Banned`) is one presence bit per entity: 1M entities fit in 16 presence pages, which stay in the buffer pool. `SELECT (Player, Inventory, Warrior) WITHOUT Banned` picks the archetypes containing `Player`, `Inventory` and `Warrior`, then skips rows whose `Banned` bit is set.

**[PLANNED]** Per-archetype row counts and per-sparse-store counts, so the planner can order filters from the most selective.

### 3.3 On-disk layout

**[DECIDED] Pages plus a buffer pool, not an LSM tree.** Compared against the references in `ref/src`:

| System | Model | Fit for ECSQL |
|---|---|---|
| PostgreSQL | heap files of 8 KiB pages; every access goes through the buffer manager (`ReadBuffer`, pin, lock, unpin; `heapam.c`); a whole row sits in one page; variable-size tuples need line pointers; updates write new versions, VACUUM cleans up | page model and one-page row access fit; we avoid line pointers and tuple growth because rows are fixed-size |
| SQLite | one file, everything is a B-tree keyed by rowid; pages via `sqlite3PagerGet` and a page cache | page model fits; we don't need a tree for entity lookups |
| ScyllaDB | LSM: in-memory memtable, flushed to immutable SSTables, merged by compaction; a row cache instead of a page cache; Seastar bypasses the OS page cache | its strength is sustained write throughput, which is not our workload; reads would have to check several files |
| Bevy | memory only; archetype tables by default, sparse sets opt-in | the same split we use, in memory |
| DuckDB | columnar: row groups of 122,880 rows, column segments in 256 KiB blocks, validity bitmaps for missing values, MVCC through version info beside the data (`src/storage/table/`) | validity bitmaps fit our sparse presence bits; its column-per-segment layout and large blocks suit analytics, not one-page entity loads |

**[DECIDED] Update in place.** A changed component overwrites its slot in the page where it already is; the page is written back at checkpoint. (PostgreSQL writes a new tuple version instead; ScyllaDB never touches existing files and merges later by compaction.) Fallback if random page writes at checkpoint ever become the bottleneck: keep changes in memory and rewrite pages in batches at checkpoint, as DuckDB does with `UpdateSegment` (`src/storage/table/`).

Why pages: an entity is `metadata[index] -> (archetype, row) -> page row / N`, one page read with no tree walk. Fixed-size rows need no line pointers and are updated in place. PAX keeps each component's column contiguous inside the page. The WAL gives sequential writes, which is most of what an LSM would buy us. From ScyllaDB we take our own I/O and buffer pool that bypass the OS page cache, and later one thread per core.

What carries over from the in-memory code: `Entity`, the allocator, the component registry, `RowLayout` (the slot format of one component inside a page), and the Table logic. Split: `ecsql-core` (`no_std`) defines the page formats and table logic over `&mut [u8]` pages; `ecsql-storage` does files, the buffer pool and the WAL.

**[DECIDED] Table page format (PAX).** Compared with PostgreSQL (24-byte header, line pointers for variable-size tuples, checksum mixed with the block number), SQLite (8-12 byte B-tree header, cell pointers, no page checksum), DuckDB (256 KiB blocks with an 8-byte checksum header, one block per column segment) and ScyllaDB (immutable compressed chunks). Fixed-size, dense rows let us drop line pointers, free-space pointers and per-page minipage offsets.

```
offset  size  field
0       8     lsn           u64  WAL position of the last change (the page is never flushed before the WAL up to here)
8       4     checksum      u32  CRC32C of the page with this field zeroed, mixed with the page number
12      4     archetype_id  u32  whose layout this page uses; makes pages self-describing
16      2     row_count     u16  rows in use: slots [0, row_count)
18      1     kind          u8   table page / sparse page / metadata page / ...
19      1     version       u8   format version
20      4     reserved
24      ...   minipages: entity IDs (N x 8 bytes), then one minipage per component (N x size),
              ordered by alignment, largest first, so each starts aligned with no padding;
              tags take no space
```
- 8 KiB pages; all header integers little-endian.
- `N = (8192 - 24) / (8 + sum of component sizes)`. Example: 88 bytes per entity gives 92 entities per page.
- Row `r` is page `r / N`, slot `r % N`; component `C` is at `offset(C) + slot * size(C)`. Offsets are computed once per archetype from the catalog, not stored in the page.
- **Maximum row size: 2 KiB** of component data per entity (at least 3 rows per page, after the header and the 8-byte entity IDs). An `ADD` that would exceed it is rejected with an error; larger components must be `SPARSE`.
- **Checksum: CRC32C from a crate** (hardware-accelerated on x86 and ARM). It is computed when a page is written to disk, so it lives in `ecsql-storage`; `ecsql-core` only defines the field.
- Rows stay dense through swap-remove; an empty last page is freed. There are never dead rows inside a page, so there is no per-row free-space map and no PostgreSQL-style VACUUM (scanning every page and fixing every index).
- **[OPEN] Cleanup still exists, at page level:** (1) once readers get snapshots (section 4), old versions must be dropped when no snapshot needs them: old page copies with copy-on-write pages, or in-memory deltas merged at checkpoint as in DuckDB; (2) freed pages leave holes in the file, so we need a free-page list for reuse and possibly a file-shrinking `VACUUM`, as SQLite has.

**[OPEN] Next:** the sparse store's presence page format.

Starting design:

- **Page-based files**, fixed page size (default 8 KiB), each with a header and checksum.
- **Catalog file**: component definitions, archetype definitions (component sets and their table IDs).
- **Table files [OPEN]**: how each archetype's pages are grouped into files.
- **Entity metadata file**: the `index -> { generation, archetype, row }` array, paged.
- **Sparse stores**: presence pages plus data pages or B-tree pages, keyed by entity index.
- **Write-ahead log (WAL)**: append-only, records every mutation. This is the durability mechanism.
- **Buffer pool [DECIDED]**: our own page cache with eviction, bypassing the OS page cache. Mmap gives up control over write ordering.

### 3.4 Archetype moves on disk

With PAX pages a move copies one row, not one page per component (section 3.1).

**Write speed [DECIDED as a goal].** Fast `ADD` / `REMOVE` of components is ECSQL's main advantage over traditional tables, so the write path is designed for it:
- A commit waits only for one sequential WAL append (plus its flush). The pages a move dirties (source page, the source's last page, the destination's last page, metadata) are changed in the buffer pool and written later, in batches, at checkpoint. Dirty pages cost background bandwidth, not commit latency.
- The pages a move touches are mostly hot: every archetype's last page is where all its inserts go, so it stays in memory.
- For comparison, a PostgreSQL `UPDATE` also writes a whole new tuple version (`heap_update`, `heapam.c`) and new index entries for every index unless the update qualifies as HOT (`README.HOT`). An ECSQL move is in the same cost range, and needs no index changes because indexes hold `Entity`.

Strategy, in order of implementation:

1. **WAL first.** A move is logged as one record. The change is durable once the log is flushed.
2. **SPARSE for components that toggle often**, so frequent add/remove never moves rows.
3. **Coalesce within a transaction.** If one transaction inserts `Inventory` and `SkillBook` on the same entity, compute the final component set and do **one** move to the final archetype, not two hops.
4. **Per-transaction batching.** A transaction that adds the same component to N entities becomes one bulk copy per source/target archetype pair, writing each touched page once.

---


## 4. Transactions and recovery

Start simple, harden later.

- **v1:** single writer, many readers. Snapshot isolation via copy-on-write pages or versioned rows **[OPEN: which]**.
- **WAL record types:** `AllocEntity`, `FreeEntity`, `DefineComponent`, `Insert(entity, component, value)`, `Remove(entity, component)`, `Update(entity, component, value)`, `Commit(txn)`.
- **Recovery:** on open, load the last checkpoint, replay committed WAL records after it, discard uncommitted ones.
- **Checkpoints:** periodically flush dirty pages, record the checkpoint LSN, truncate the WAL.
- **Crash testing is mandatory:** fault-injection tests that kill the process at random points in write paths and verify recovery (see section 8).
- **[OPEN]** multiple concurrent writers, and conflict detection granularity (per entity, per archetype). Because ECSQL is a server with many clients, this matters earlier than it would for an embedded database.

---

## 5. Query language (ECSQL)

SQL-flavoured, with the ID-join removed from the syntax. A statement targets a database (`IN MYGAMEDB`). **[OPEN]** exact keyword set and grammar. Treat the examples below as the spec for the first parser.

### 5.1 Entity lifecycle

```sql
-- allocate an ID first (separate step)
e = SPAWN IN MYGAMEDB;

-- attach components to it
INSERT (Inventory(), SkillBook()) FOR e IN MYGAMEDB;

-- one-statement sugar: allocate and insert
SPAWN (Player(), Position(0, 0), Inventory()) IN MYGAMEDB;

-- different kinds of entity are just different component sets, no NULL columns
SPAWN (Warrior(), Player(), Stats()) IN MYGAMEDB;
SPAWN (Mage(), Player(), MagicStats()) IN MYGAMEDB;

-- delete an entity (frees the ID, bumps the generation)
DESPAWN e IN MYGAMEDB;
```

Rules:
- A freshly spawned entity lives in the **empty archetype** until components are inserted.
- Components passed in a single `INSERT` produce a **single** archetype move.
- **[OPEN]** Does `e` bind a variable for the whole session or transaction, or is `FOR` always given a literal ID? Prefer a transaction-scoped binding.

### 5.2 Reads

```sql
SELECT Position, Health IN MYGAMEDB;
SELECT Position WITHOUT Dead IN MYGAMEDB;
SELECT Name, Health.current WHERE Health.current < 10 IN MYGAMEDB;
SELECT AVG(Health.current) WITH Position IN MYGAMEDB;
SELECT Name WHERE Parent.id = 4182 IN MYGAMEDB;                 -- relation lookup
SELECT Entity, Player WHERE Name = "Username" IN MYGAMEDB;      -- login lookup
GET e: Position, Health IN MYGAMEDB;                            -- fetch one entity by ID
SELECT (Player, Inventory, Warrior) WITHOUT Banned IN MYGAMEDB; -- tags narrow the search
```

- **[DECIDED] There is no `FROM` clause.** There are no tables to choose from: the components named in `SELECT`, `WITH`, and field references in `WHERE` form the required component set, and every entity that has all of them is a candidate.
- **[DECIDED]** `Entity` is a pseudo-column: the entity ID stored in every table.
- **[OPEN]** Shorthand for single-field components: `Name = x` meaning `Name.value = x`, an error when the component has more than one field.
- `WITHOUT` excludes archetypes containing the listed components, with no per-row check.
- There is no `JOIN` keyword in v1.
- A tag in the `SELECT` list acts as a filter only: it has no fields to return.
- The syntax is not designed yet: the examples in this section illustrate the idea, not the final grammar.

### 5.3 Writes and structural changes

```sql
UPDATE Health SET current = current - 5 WHERE Health.current > 0 IN MYGAMEDB;
ADD Stunned WHERE Health.current < 10 IN MYGAMEDB;          -- bulk structural change
REMOVE Stunned WHERE Stunned.until < NOW() IN MYGAMEDB;
```

**[OPEN]** exact form of `ADD` / `REMOVE` now that there is no `entities` source.

### 5.4 DDL

```sql
CREATE DATABASE MYGAMEDB;
CREATE COMPONENT Position (x f32, y f32);
DROP COMPONENT Position;                 -- [OPEN] semantics: remove from all entities, rebuild archetypes
ALTER COMPONENT Health ADD FIELD regen f32 DEFAULT 0;   -- [OPEN]
```

### 5.5 Query planning

The planner is small by design:
1. Resolve component names to `ComponentId`s.
2. Build the `with` and `without` sets.
3. Look up or create a cached query, with its incrementally maintained archetype match list.
4. Choose the access path per matching archetype: a page scan of its table, a sparse presence check, or a secondary index lookup.
5. Push filters down into the column scan. Prefer filtering on the most selective column first.

**Secondary indexes [PLANNED]:** **[DECIDED]** explicit and opt-in, declared by the user (`CREATE INDEX ON Name`, optionally UNIQUE), as in PostgreSQL. Every index costs work on each insert or change of its component, so the database never adds one on its own. A B-tree by default, as in PostgreSQL (`nbtree`, Lehman & Yao), because it also serves ranges, prefixes, `ORDER BY` and UNIQUE, not just equality. Unlike PostgreSQL, whose index entries hold a physical TID (`ItemPointerData { block, offset }`), our leaves hold the `Entity`, so `swap_remove` and archetype moves never touch an index; only a change to the indexed value does. A login is then `index -> Entity -> metadata -> (archetype, row) -> one page`. **[OPEN]** spatial indexes on `Position`.

---

## 6. Client/server architecture

ECSQL runs as a server process that owns the data. Clients connect over TCP and send statements, like `psql` or a Postgres driver talking to PostgreSQL.

```
client (game server) --TCP--> ecsql-server --> ecsql-lang (parse, plan)
                                            --> ecsql-storage (WAL, pages)
                                            --> ecsql-core (archetypes, queries)
```

### Rust client library

```rust
let client = Client::connect("ecsql://host/MYGAMEDB").await?;

client.execute("CREATE COMPONENT Position (x f32, y f32)").await?;

let mut tx = client.begin().await?;
let e = tx.spawn().await?;                             // allocate ID
tx.insert(e, &[("Inventory", &[]), ("SkillBook", &[])]).await?;
tx.commit().await?;

for row in client.query("SELECT Position, Health").await? {
    // row.entity, row.get::<f32>("Position.x"), ...
}
```

- Every call is a network round trip, so set-based statements (`UPDATE ... WHERE`, `ADD ... WHERE`) matter more than per-entity calls.
- Block ID reservation (section 2) lets a client allocate a batch of entity IDs in one round trip, then spawn locally.
- The server drives the engine through one query IR. The SQL-ish string interface and any typed builder API both compile to it.
- **[OPEN]** wire protocol: a custom binary protocol, or the PostgreSQL wire protocol so existing tools (`psql`, drivers) can connect.
- **[OPEN]** a derive macro for statically typed access from Rust (`#[derive(Component)]`) that registers the schema and gives typed reads without strings. Nice-to-have for the game server.

---

## 7. Project structure (suggested Cargo workspace)

```
ecsql/
 ├─ ecsql-core/      # no_std + alloc: entities, components, archetypes, tables, in-memory queries
 ├─ ecsql-storage/   # pages, buffer pool, WAL, checkpoints, compaction, recovery
 ├─ ecsql-lang/      # lexer, parser, AST, planner, query IR
 ├─ ecsql-server/    # server process: wire protocol, sessions, transactions per connection
 ├─ ecsql-client/    # Rust client library
 ├─ ecsql-cli/       # REPL (in-process engine during early milestones, client later)
 └─ ecsql-bench/     # benchmarks (criterion) and comparison harnesses
```

Crate name: **ecsql**. Check crates.io and GitHub availability first. Fallbacks: `ecsql-db`, `ecsql-core`.

---

## 8. Milestones

> **[DECIDED]** Because the workload is storage first (section 1), the page format and buffer pool come right after the core data structures, instead of waiting for the whole in-memory store and the benchmark gate. The list below is the original plan; progress so far:
> - Done in `ecsql-core`: `Entity` and the entity allocator, the component registry, `RowLayout`, `Column` (aligned type-erased rows, checked with Miri), `Table` (push, reserve, swap_remove, move_row_to).
> - **[DECIDED]** Storage is archetype tables in PAX pages, with opt-in sparse stores (section 3.1). The `Table` code maps to one page's operations.
> - Next: the presence and data page formats, then the buffer pool and WAL, then the World on top of them.

1. **M1: In-memory archetype store** (`ecsql-core`)
   - Runtime-defined components, spawn/despawn, insert/remove, edges cache.
   - Query by component set (`with`/`without`) over matching archetypes, with incremental match lists.
   - Unit tests, plus property tests comparing against a naive model (a `HashMap<Entity, HashMap<Component, Value>>`).
2. **M2: Benchmark gate**
   - "AVG(Health) over everything with Position and Health", at 10k / 1M / 10M entities, with varying archetype counts (fragmentation).
   - Compare against DuckDB and, as a baseline, `bevy_ecs`.
   - **Decision point:** if the speed story does not hold, rethink before building the disk layer.
3. **M3: Query language**
   - Lexer, parser, AST, planner for the section 5 examples. REPL.
4. **M4: Persistence**
   - Pages, WAL, checkpoint, recovery. Single writer.
   - Fault-injection crash tests.
5. **M5: Archetype-move optimizations**
   - Per-transaction coalescing and batching, sparse stores for toggled components, move plans per archetype edge.
6. **M6: Transactions and concurrency**
   - Snapshot isolation, concurrent readers, then (maybe) multiple writers.
7. **M7: Server**
   - `ecsql-server` with a wire protocol, per-connection sessions, and `ecsql-client`.
   - The REPL switches to connecting through the client.
8. **M8: Game-server proving ground**
   - Back an MMO server's persistence with ECSQL. Measure real workloads.

---

## 9. Testing and benchmarking requirements

- **Model-based property tests:** random operation sequences applied to ECSQL and to a trivially correct reference model must yield identical query results. Run on every milestone.
- **Crash/recovery tests:** kill at random points during writes, reopen, and verify committed data is present and uncommitted data is absent.
- **Invariants to assert in debug builds:**
  - `entities[i].archetype/row` always points at a table row whose entity column holds that entity.
  - Every column in a table has the same length as the table's entity column.
  - A sparse component's presence bit is set exactly for the entities that have it.
  - Archetype component sets are sorted and unique.
- **Benchmarks** (criterion): scan one column, scan many columns, query across many archetypes, bulk add/remove component, spawn throughput, recovery time versus WAL size.

---

## 10. Open questions (collected)

- Final query syntax: keywords, variable binding for `FOR e`, whether to ever support explicit `JOIN`.
- Isolation model and writer concurrency.
- On-disk column encoding: compression (dictionary, RLE, bit-packing) and when to introduce it.
- Archetype explosion: many rarely used component combinations produce many tiny tables. Possible mitigations: merge small tables, cap archetype count, move rare components to sparse storage automatically.
- Secondary indexes and spatial queries.
- Schema evolution (`ALTER COMPONENT`, `DROP COMPONENT`) and its effect on archetypes.
- Column page format: header fields, row placement, free space.
- Wire protocol: custom binary, or PostgreSQL wire-protocol compatible.

---

## 11. Notes for the implementer (Claude Code)

- Storage first: every structure must also map to pages and WAL records (fixed byte order, fixed alignment, fixed-size records, offsets instead of pointers, stable IDs instead of physical positions). Disk code starts right after the core data structures, not after the whole in-memory store.
- Keep component data as untyped byte columns with a runtime layout. Avoid generic-over-component designs in the core.
- For every topic, compare all the references before deciding, and record the comparison here. Shallow clones live in `ref/src` (gitignored): PostgreSQL, SQLite, ScyllaDB, Seastar, Bevy; papers in `ref/books`. Also worth reading: `flecs` (runtime-defined components, query language), DuckDB (columnar execution). Read for ideas, do not copy blindly, and check each project's license before reusing code.
- Prefer small, well-tested steps over large rewrites. After each milestone, record benchmark numbers in `BENCHMARKS.md`.
- Update this document as each step is decided and built.

---

## 12. Decision log

| Date | Decision |
|---|---|
| 2026-11-17 | `Entity` is a packed `u64` with a `NonZeroU32` generation. |
| 2026-11-17 | One column per component (whole values); row fields stable-sorted by alignment, largest first; declaration order is identity. |
| 2026-11-18 | `Table` uses sorted parallel arrays, not a `HashMap`; `swap_remove` and `move_row_to` return the moved entity; `move_row_to` takes the gained components' rows in destination ID order and does not validate them (caller contract). |
| 2026-11-18 | Entity lookup is through the metadata array; indexes store `Entity`, never a physical position. |
| 2026-11-18 | Query syntax has no `FROM`; `Entity` is a pseudo-column. |
| 2026-11-18 | Workload is storage first: the game server holds live state, ECSQL persists it. |
| 2026-11-18 | Storage is pages + our own buffer pool (PostgreSQL/SQLite style), WAL; not an LSM tree. |
| 2026-11-18 | ECSQL is transactional (OLTP) and dynamic first, and not only for games; analytics must work but need not be the fastest. |
| 2026-11-18 | Entity-indexed component stores replace physical archetype tables: one paged store per component, slot derived from the entity index, presence bitmaps; archetypes are logical signatures only. Adding or removing a component touches one page and nothing ever moves. The cost is density, handled by lowest-free allocation and allocation pools. |
| 2026-11-18 | **Reversed the previous row.** Archetype tables stay the default, stored as PAX pages (whole rows per page, each component's column contiguous inside the page): loading a whole entity is 1 page instead of one per component, and a move copies one row (about 5 pages), which is acceptable because add/remove is occasional. The earlier cost estimate wrongly assumed column-chunked pages. Entity-indexed stores with presence bits become the opt-in `SPARSE` kind for components that toggle often (as Bevy's `Table` default and `SparseSet` opt-in). |
| 2026-11-18 | Updates replace a whole component value, not single fields: the logic lives in the component. |
| 2026-11-18 | Regular pages with update in place (like PostgreSQL, SQLite, InnoDB, DuckDB), not an LSM. PAX table page: 24-byte header (lsn, CRC32C checksum, archetype_id, row_count, kind, version), minipages ordered by alignment, 2 KiB maximum row size. |
| 2026-11-18 | Relations: owned things are separate entities, and the link lives on the owned entity (`ItemOwner(owner entity)`), queried through an index. Indexes are explicit and declared by the user. |

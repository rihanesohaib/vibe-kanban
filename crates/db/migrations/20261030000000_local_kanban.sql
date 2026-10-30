-- Local kanban: projects become boards with statuses, issues, tags, relationships
-- and comments stored in SQLite. Legacy tasks are migrated into issues keeping
-- their ids, so workspaces.task_id keeps pointing at the same work item.

ALTER TABLE projects ADD COLUMN color TEXT NOT NULL DEFAULT '217 91% 60%';
ALTER TABLE projects ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
ALTER TABLE projects ADD COLUMN issue_prefix TEXT NOT NULL DEFAULT 'ISS';
ALTER TABLE projects ADD COLUMN issue_counter INTEGER NOT NULL DEFAULT 0;

UPDATE projects
SET issue_prefix = COALESCE(
        NULLIF(UPPER(SUBSTR(REPLACE(REPLACE(REPLACE(name, ' ', ''), '-', ''), '_', ''), 1, 3)), ''),
        'ISS'
    ),
    sort_order = (
        SELECT COUNT(*) FROM projects p2
        WHERE p2.created_at < projects.created_at
           OR (p2.created_at = projects.created_at AND p2.id < projects.id)
    );

CREATE TABLE project_statuses (
    id          BLOB PRIMARY KEY,
    project_id  BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    color       TEXT NOT NULL,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    hidden      INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
CREATE INDEX idx_project_statuses_project_id ON project_statuses(project_id, sort_order);

INSERT INTO project_statuses (id, project_id, name, color, sort_order, hidden)
SELECT randomblob(16), p.id, d.name, d.color, d.sort_order, d.hidden
FROM projects p
CROSS JOIN (
    SELECT 'Backlog' AS name, '220 9% 46%' AS color, 0 AS sort_order, 1 AS hidden
    UNION ALL SELECT 'To do', '217 91% 60%', 1, 0
    UNION ALL SELECT 'In progress', '38 92% 50%', 2, 0
    UNION ALL SELECT 'In review', '258 90% 66%', 3, 0
    UNION ALL SELECT 'Done', '142 71% 45%', 4, 0
    UNION ALL SELECT 'Cancelled', '0 84% 60%', 5, 1
) d;

CREATE TABLE issues (
    id                      BLOB PRIMARY KEY,
    project_id              BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    issue_number            INTEGER NOT NULL,
    simple_id               TEXT NOT NULL,
    status_id               BLOB NOT NULL REFERENCES project_statuses(id),
    title                   TEXT NOT NULL,
    description             TEXT,
    priority                TEXT CHECK (priority IN ('urgent', 'high', 'medium', 'low')),
    start_date              TEXT,
    target_date             TEXT,
    completed_at            TEXT,
    sort_order              REAL NOT NULL DEFAULT 0,
    parent_issue_id         BLOB REFERENCES issues(id) ON DELETE SET NULL,
    parent_issue_sort_order REAL,
    extension_metadata      TEXT NOT NULL DEFAULT '{}',
    creator_user_id         BLOB,
    created_at              TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    updated_at              TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (project_id, issue_number)
);
CREATE INDEX idx_issues_project_id ON issues(project_id);
CREATE INDEX idx_issues_status_id ON issues(status_id);
CREATE INDEX idx_issues_parent_issue_id ON issues(parent_issue_id);

INSERT INTO issues (
    id, project_id, issue_number, simple_id, status_id, title, description,
    completed_at, sort_order, created_at, updated_at
)
SELECT t.id,
       t.project_id,
       n.num,
       p.issue_prefix || '-' || n.num,
       s.id,
       t.title,
       t.description,
       CASE WHEN t.status = 'done' THEN t.updated_at END,
       n.num,
       t.created_at,
       t.updated_at
FROM (
    SELECT id, ROW_NUMBER() OVER (PARTITION BY project_id ORDER BY created_at, id) AS num
    FROM tasks
) n
JOIN tasks t ON t.id = n.id
JOIN projects p ON p.id = t.project_id
JOIN project_statuses s
  ON s.project_id = t.project_id
 AND s.name = CASE t.status
                  WHEN 'todo' THEN 'To do'
                  WHEN 'inprogress' THEN 'In progress'
                  WHEN 'inreview' THEN 'In review'
                  WHEN 'done' THEN 'Done'
                  ELSE 'Cancelled'
              END;

UPDATE projects
SET issue_counter = (
    SELECT COALESCE(MAX(issue_number), 0) FROM issues WHERE issues.project_id = projects.id
);

CREATE TABLE kanban_tags (
    id          BLOB PRIMARY KEY,
    project_id  BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    color       TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
CREATE INDEX idx_kanban_tags_project_id ON kanban_tags(project_id);

CREATE TABLE issue_tags (
    id        BLOB PRIMARY KEY,
    issue_id  BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    tag_id    BLOB NOT NULL REFERENCES kanban_tags(id) ON DELETE CASCADE,
    UNIQUE (issue_id, tag_id)
);
CREATE INDEX idx_issue_tags_tag_id ON issue_tags(tag_id);

CREATE TABLE issue_relationships (
    id                BLOB PRIMARY KEY,
    issue_id          BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    related_issue_id  BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    relationship_type TEXT NOT NULL CHECK (relationship_type IN ('blocking', 'related', 'has_duplicate')),
    created_at        TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (issue_id, related_issue_id, relationship_type)
);
CREATE INDEX idx_issue_relationships_related ON issue_relationships(related_issue_id);

CREATE TABLE issue_comments (
    id          BLOB PRIMARY KEY,
    issue_id    BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    author_id   BLOB,
    parent_id   BLOB REFERENCES issue_comments(id) ON DELETE CASCADE,
    message     TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
CREATE INDEX idx_issue_comments_issue_id ON issue_comments(issue_id, created_at);

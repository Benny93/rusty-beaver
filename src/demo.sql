CREATE TABLE lodges (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  river TEXT NOT NULL,
  built_year INTEGER
);
CREATE TABLE beavers (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  rank TEXT NOT NULL,
  lodge_id INTEGER REFERENCES lodges(id),
  logs_chewed INTEGER DEFAULT 0,
  joined DATE
);
CREATE TABLE missions (
  id INTEGER PRIMARY KEY,
  codename TEXT NOT NULL,
  leader_id INTEGER REFERENCES beavers(id),
  status TEXT CHECK (status IN ('planned','active','complete','failed')),
  dams_built INTEGER DEFAULT 0
);
INSERT INTO lodges VALUES
  (1,'Fort Timber','Yukon',1998),(2,'Splash Base','Mississippi',2004),
  (3,'Log Cabin Prime','Danube',2011),(4,'Outpost Chomp','Rhine',2019);
INSERT INTO beavers VALUES
  (1,'Marco','Commander',1,15230,'2001-04-12'),(2,'Tarma','Sergeant',1,9870,'2003-08-01'),
  (3,'Eri','Specialist',2,12044,'2005-02-17'),(4,'Fio','Medic',2,7311,'2006-11-30'),
  (5,'Bucky','Private',3,2210,'2015-06-05'),(6,'Chompers','Private',3,3105,'2016-01-22'),
  (7,'Timber','Engineer',4,8800,'2019-09-09'),(8,'Nibbles','Recruit',4,140,'2024-03-14');
INSERT INTO missions VALUES
  (1,'Operation Deep Dam',1,'complete',12),(2,'Midnight Gnaw',2,'complete',5),
  (3,'River Rush',3,'active',3),(4,'Silent Splash',7,'planned',0),
  (5,'Twig Storm',5,'failed',1),(6,'Last Log Standing',1,'active',7);
CREATE VIEW leaderboard AS
  SELECT b.name, b.rank, l.name AS lodge, b.logs_chewed
  FROM beavers b JOIN lodges l ON l.id = b.lodge_id
  ORDER BY b.logs_chewed DESC;

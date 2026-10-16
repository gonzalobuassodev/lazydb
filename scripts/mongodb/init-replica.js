const hello = db.adminCommand({ hello: 1 });
if (!hello.setName) {
  rs.initiate({
    _id: "lazydb-rs",
    members: [{ _id: 0, host: "mongodb-rs:27017" }],
  });
}

#![feature(custom_attribute)]
#![feature(type_ascription)]
#![feature(associated_consts)]
#![feature(proc_macro)]

#[macro_use(message, storage_value)]
extern crate exonum;
extern crate serde;
extern crate byteorder;
extern crate blockchain_explorer;
extern crate geo;
extern crate time;
#[macro_use]
extern crate serde_derive;

mod txs;
mod view;
pub mod cors;
pub mod api;

use exonum::storage::{Map, Database, Error, List};
use exonum::blockchain::{Blockchain};
use exonum::messages::Message;
use exonum::crypto::{Hash, hash};
use std::u64;
use std::ops::Deref;

use geo::algorithm::intersects::Intersects;

pub use txs::{ObjectTx, TxCreateOwner, TxCreateObject, TxModifyObject,
              TxTransferObject, TxRemoveObject, TxRestoreObject, TxRegister, GeoPoint, timestamp};
pub use view::{ObjectsView, Owner, Object, User, ObjectId, Ownership, TxResult, ObjectHistory};

#[derive(Clone)]
pub struct ObjectsBlockchain<D: Database> {
    pub db: D,
}

impl<D: Database> Deref for ObjectsBlockchain<D> {
    type Target = D;

    fn deref(&self) -> &D {
        &self.db
    }
}

impl<D> Blockchain for ObjectsBlockchain<D> where D: Database
{
    type Database = D;
    type Transaction = ObjectTx;
    type View = ObjectsView<D::Fork>;

    fn verify_tx(tx: &Self::Transaction) -> bool {
        tx.verify(tx.pub_key())
    }

    fn state_hash(view: &Self::View) -> Result<Hash, Error> {
        let mut hashes = Vec::new();
        hashes.extend_from_slice(view.owners().root_hash()?.as_ref());
        hashes.extend_from_slice(view.objects().root_hash()?.as_ref());
        Ok(hash(&hashes))
    }

    fn execute(view: &Self::View, tx: &Self::Transaction) -> Result<(), Error> {

        match *tx {

            ObjectTx::Register(ref tx) => {
                if let Some(_) = view.users().get(tx.pub_key())? {
                    let _ = view.results().put(&tx.hash(), TxResult::register_already_registered());
                    return Ok(());
                }
                let user = User::new(tx.name());
                let _ = view.users().put(tx.pub_key(), user);
            }

            ObjectTx::CreateOwner(ref tx) => {
                let owner = Owner::new(tx.firstname(), tx.lastname(), &hash(&[]));
                view.owners().append(owner)?;
            }

            ObjectTx::CreateObject(ref tx) => {

                let objects = view.objects();

                if let Some(_) = view.owners().get(tx.owner_id())? {

                    let points = GeoPoint::from_vec(tx.points().to_vec());
                    if points.len() < 3 {
                        // At least 3 points should be defined
                        let _ = view.results().put(&tx.hash(), TxResult::create_object_wrong_points());
                        return Ok(());
                    }

                    let ls_new = GeoPoint::to_polygon(points);
                    for stored_object in objects.values()? {
                        let stored_points = GeoPoint::from_vec(stored_object.points().to_vec());
                        if ls_new.intersects(&GeoPoint::to_polygon(stored_points)) {
                            // Cross titles detected
                            let _ = view.results().put(&tx.hash(), TxResult::create_object_cross_neighbours());
                            return Ok(());
                        }
                    }

                    let object_id = objects.len()? as u64;

                    // update ownership hash
                    let owner_objects = view.owner_objects(tx.owner_id());
                    owner_objects.append(Ownership::new(object_id, true))?;
                    let new_ownership_hash = owner_objects.root_hash()?;
                    let mut owner = view.owners().get(tx.owner_id())?.unwrap();
                    owner.set_ownership_hash(&new_ownership_hash);
                    view.owners().set(tx.owner_id(), owner)?;

                    // update object history hash
                    let object_history = view.object_history(object_id);
                    object_history.append(ObjectHistory::new_create_action(tx.owner_id(), tx.owner_id(), timestamp(), &tx.hash()))?;
                    let new_history_hash = object_history.root_hash()?;

                    // insert object
                    let _ = view.results().put(&tx.hash(), TxResult::ok());
                    let object = Object::new(tx.title(), tx.points(), tx.owner_id(), false, &new_history_hash);
                    objects.append(object)?;
                } else {
                    //Owner not found by id
                    let _ = view.results().put(&tx.hash(), TxResult::create_object_wrong_owner());
                    return Ok(());
                }
            }

            ObjectTx::ModifyObject(ref tx) => {

                if let Some(mut object) = view.objects().get(tx.object_id())? {
                    // update object history hash
                    let object_history = view.object_history(tx.object_id());
                    object_history.append(ObjectHistory::new_modify_action(object.owner_id(), object.owner_id(), timestamp(), &tx.hash()))?;
                    let new_history_hash = object_history.root_hash()?;

                    // update object
                    object.set_title(tx.title());
                    object.set_points(tx.points());
                    object.set_history_hash(&new_history_hash);
                    view.objects().set(tx.object_id(), object)?;
                }else{
                    //Object not found by id
                    let _ = view.results().put(&tx.hash(), TxResult::modify_object_wrong_object());
                    return Ok(());
                }

            }

            ObjectTx::TransferObject(ref tx) => {

                if let Some(mut object) = view.objects().get(tx.object_id())? {

                    if let Some(_) = view.owners().get(tx.owner_id())? {
                        // update ownership hash
                        let old_owner_objects = view.owner_objects(object.owner_id());
                        old_owner_objects.append(Ownership::new(tx.object_id(), false))?;
                        let old_ownership_hash = old_owner_objects.root_hash()?;

                        let new_owner_objects = view.owner_objects(tx.owner_id());
                        new_owner_objects.append(Ownership::new(tx.object_id(), true))?;
                        let new_ownership_hash = new_owner_objects.root_hash()?;

                        // update owners states
                        let mut old_owner = view.owners().get(object.owner_id())?.unwrap();
                        old_owner.set_ownership_hash(&old_ownership_hash);
                        view.owners().set(object.owner_id(), old_owner)?;

                        let mut new_owner = view.owners().get(tx.owner_id())?.unwrap();
                        new_owner.set_ownership_hash(&new_ownership_hash);
                        view.owners().set(tx.owner_id(), new_owner)?;

                        // update object history hash
                        let object_history = view.object_history(tx.object_id());
                        object_history.append(ObjectHistory::new_transfer_action(object.owner_id(), tx.owner_id(), timestamp(), &tx.hash()))?;
                        let new_history_hash = object_history.root_hash()?;

                        // update object
                        object.set_owner(tx.owner_id());
                        object.set_history_hash(&new_history_hash);
                        view.objects().set(tx.object_id(), object)?;
                    }else{
                        // Owner not found
                        let _ = view.results().put(&tx.hash(), TxResult::transfer_object_wrong_owner());
                        return Ok(());
                    }

                }else{
                    // Object not found by id
                    let _ = view.results().put(&tx.hash(), TxResult::transfer_object_wrong_object());
                    return Ok(());
                }

            }

            ObjectTx::RemoveObject(ref tx) => {
                if let Some(mut object) = view.objects().get(tx.object_id())? {

                        // update object history hash
                        let object_history = view.object_history(tx.object_id());
                        object_history.append(ObjectHistory::new_remove_action(object.owner_id(), object.owner_id(), timestamp(), &tx.hash()))?;
                        let new_history_hash = object_history.root_hash()?;

                        // update object
                        object.set_deleted(true);
                        object.set_history_hash(&new_history_hash);
                        view.objects().set(tx.object_id(), object)?;

                }else{
                    // Object not found by id
                    let _ = view.results().put(&tx.hash(), TxResult::remove_object_wrong_object());
                    return Ok(());
                }
            }

            ObjectTx::RestoreObject(ref tx) => {
                if let Some(mut object) = view.objects().get(tx.object_id())? {

                        // update object history hash
                        let object_history = view.object_history(tx.object_id());
                        object_history.append(ObjectHistory::new_restore_action(object.owner_id(), object.owner_id(), timestamp(), &tx.hash()))?;
                        let new_history_hash = object_history.root_hash()?;

                        // update object
                        object.set_deleted(false);
                        object.set_history_hash(&new_history_hash);
                        view.objects().set(tx.object_id(), object)?;

                }else{
                    // Object not found by id
                    let _ = view.results().put(&tx.hash(), TxResult::restore_object_wrong_object());
                    return Ok(());
                }
            }

        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use super::ObjectsBlockchain;
    use exonum::messages::Message;
    use view::{ObjectsView, TxResult};
    use txs::{TxCreateOwner, TxCreateObject, TxModifyObject, TxTransferObject, TxRemoveObject, ObjectTx, TxRegister, timestamp};
    use exonum::crypto::{gen_keypair};
    use exonum::blockchain::Blockchain;
    use exonum::storage::{Map, List, Database, MemoryDB, Result as StorageResult};

    fn execute_tx<D: Database>(v: &ObjectsView<D::Fork>,
                               tx: ObjectTx)
                               -> StorageResult<()> {
        ObjectsBlockchain::<D>::execute(v, &tx)
    }

    #[test]
    fn register(){
        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let (p, s) = gen_keypair();
        let tx_register = TxRegister::new(&p, "test user", &s);

        // Act
        let ok_result = execute_tx::<MemoryDB>(&v, ObjectTx::Register(tx_register.clone()));
        let stored_user = v.users().get(&&p).unwrap().unwrap();
        let err_result = execute_tx::<MemoryDB>(&v, ObjectTx::Register(tx_register.clone()));

        // Assert
        assert_eq!(ok_result.is_ok(), true);
        assert_eq!(err_result.is_ok(), true);
        let r = v.results().get(&tx_register.hash()).unwrap().unwrap();
        assert_eq!(r.result(), TxResult::RESULT_ALREADY_REGISTERED);
        assert_eq!(stored_user.name(), "test user");
    }

    #[test]
    fn test_add_owner() {

        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let (p, s) = gen_keypair();
        let owner_id = 0_u64;
        let tx_create_owner = TxCreateOwner::new(&p, "firstname", "lastname", &s);

        // Act
        let ok_result = execute_tx::<MemoryDB>(&v, ObjectTx::CreateOwner(tx_create_owner.clone()));
        let stored_owner = v.owners().get(owner_id).unwrap().unwrap();

        // Assert
        assert_eq!(ok_result.is_ok(), true);
        assert_eq!(stored_owner.firstname(), "firstname");
        assert_eq!(stored_owner.lastname(), "lastname");

    }

    #[test]
    fn test_add_object_to_wrong_owner_fails(){
        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let (p, s) = gen_keypair();

        let wrong_owner_id = 0_u64;

        let points = vec![1.0, 2.0, 3.0, 4.0];
        let tx_create_object = TxCreateObject::new(&p, "test object title", &points, wrong_owner_id, &s);

        // Act
        let create_object_result = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object.clone()));

        // Assert
        assert_eq!(create_object_result.is_ok(), true);
        assert_eq!(v.objects().len().unwrap(), 0);
        let r = v.results().get(&tx_create_object.hash()).unwrap().unwrap();
        assert_eq!(r.result(), TxResult::RESULT_CREATE_WRONG_OWNER);

    }

    #[test]
    fn test_add_object_with_2_points_fails(){
        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let (p, s) = gen_keypair();

        let owner_id = 0_u64;
        let tx_create_owner = TxCreateOwner::new(&p, "firstname", "lastname", &s);
        let create_owner_result = execute_tx::<MemoryDB>(&v, ObjectTx::CreateOwner(tx_create_owner.clone()));

        let points = vec![1.0, 2.0, 3.0, 4.0];
        let tx_create_object = TxCreateObject::new(&p, "test object title", &points, owner_id, &s);

        // Act
        let create_object_result = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object.clone()));

        // Assert
        assert_eq!(create_owner_result.is_ok(), true);
        assert_eq!(create_object_result.is_ok(), true);
        let r = v.results().get(&tx_create_object.hash()).unwrap().unwrap();
        assert_eq!(r.result(), TxResult::RESULT_WRONG_POINTS);


    }

    #[test]
    fn test_add_object_crossing_another_fails(){
        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let (p, s) = gen_keypair();

        let owner_id = 0_u64;
        let tx_create_owner = TxCreateOwner::new(&p, "firstname", "lastname", &s);
        let create_owner_result = execute_tx::<MemoryDB>(&v, ObjectTx::CreateOwner(tx_create_owner.clone()));

        let points1 = vec![0.0, 0.0, 2.0, 0.0, 2.0, 2.0, 0.0, 2.0];
        let tx_create_object1 = TxCreateObject::new(&p, "test object title1", &points1, owner_id, &s);

        let points2 = vec![1.0, 1.0, 3.0, 1.0, 3.0, 3.0, 0.0, 3.0];
        let tx_create_object2 = TxCreateObject::new(&p, "test object title2", &points2, owner_id, &s);


        // Act
        let create_object_result1 = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object1.clone()));
        let create_object_result2 = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object2.clone()));

        // Assert
        assert_eq!(create_owner_result.is_ok(), true);
        assert_eq!(create_object_result1.is_ok(), true);
        assert_eq!(create_object_result2.is_ok(), true);
        let r = v.results().get(&tx_create_object2.hash()).unwrap().unwrap();
        assert_eq!(r.result(), TxResult::RESULT_WRONG_NEIGHBOURS);


    }

    #[test]
    fn test_add_object_the_same_to_another_fails(){
        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let (p, s) = gen_keypair();

        let owner_id = 0_u64;
        let tx_create_owner = TxCreateOwner::new(&p, "firstname", "lastname", &s);
        let create_owner_result = execute_tx::<MemoryDB>(&v, ObjectTx::CreateOwner(tx_create_owner.clone()));

        let points1 = vec![0.0, 0.0, 2.0, 0.0, 2.0, 2.0, 0.0, 2.0];
        let tx_create_object1 = TxCreateObject::new(&p, "test object title1", &points1, owner_id, &s);

        let points2 = vec![0.0, 0.0, 2.0, 0.0, 2.0, 2.0, 0.0, 2.0];
        let tx_create_object2 = TxCreateObject::new(&p, "test object title2", &points2, owner_id, &s);


        // Act
        let create_object_result1 = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object1.clone()));
        let create_object_result2 = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object2.clone()));

        // Assert
        assert_eq!(create_owner_result.is_ok(), true);
        assert_eq!(create_object_result1.is_ok(), true);
        assert_eq!(create_object_result2.is_ok(), true);
        let r = v.results().get(&tx_create_object2.hash()).unwrap().unwrap();
        assert_eq!(r.result(), TxResult::RESULT_WRONG_NEIGHBOURS);


    }

    #[test]
    fn test_add_object_inside_another_fails(){
        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let (p, s) = gen_keypair();

        let owner_id = 0_u64;
        let tx_create_owner = TxCreateOwner::new(&p, "firstname", "lastname", &s);
        let create_owner_result = execute_tx::<MemoryDB>(&v, ObjectTx::CreateOwner(tx_create_owner.clone()));

        let points1 = vec![0.0, 0.0, 2.0, 0.0, 2.0, 2.0, 0.0, 2.0];
        let tx_create_object1 = TxCreateObject::new(&p, "test object title1", &points1, owner_id, &s);

        let points2 = vec![0.5, 0.5, 1.5, 0.5, 1.5, 1.5, 0.5, 1.5];
        let tx_create_object2 = TxCreateObject::new(&p, "test object title2", &points2, owner_id, &s);


        // Act
        let create_object_result1 = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object1.clone()));
        let create_object_result2 = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object2.clone()));

        // Assert
        assert_eq!(create_owner_result.is_ok(), true);
        assert_eq!(create_object_result1.is_ok(), true);
        assert_eq!(create_object_result2.is_ok(), true);
        let r = v.results().get(&tx_create_object2.hash()).unwrap().unwrap();
        assert_eq!(r.result(), TxResult::RESULT_WRONG_NEIGHBOURS);

    }

    #[test]
    fn test_add_object(){

        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let (p, s) = gen_keypair();

        let owner_id = 0_u64;

        let tx_create_owner = TxCreateOwner::new(&p, "firstname", "lastname", &s);

        let object_id1 = 0_u64;
        let points1 = vec![1.0, 1.0, 3.0, 1.0, 3.0, 4.0];
        let tx_create_object1 = TxCreateObject::new(&p, "test object title1", &points1, owner_id, &s);

        let object_id2 = 1_u64;
        let points2 = vec![4.0, 2.0, 5.0, 2.0, 5.0, 4.0];
        let tx_create_object2 = TxCreateObject::new(&p, "test object title2", &points2, owner_id, &s);


        // Act
        let create_owner_result = execute_tx::<MemoryDB>(&v, ObjectTx::CreateOwner(tx_create_owner.clone()));
        let create_object_result1 = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object1.clone()));
        let create_object_result2 = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object2.clone()));

        //Assert
        assert_eq!(create_owner_result.is_ok(), true);
        assert_eq!(create_object_result1.is_ok(), true);
        assert_eq!(create_object_result2.is_ok(), true);

        let stored_object1 = v.objects().get(object_id1).unwrap().unwrap();
        let stored_object2 = v.objects().get(object_id2).unwrap().unwrap();
        let stored_owner = v.owners().get(owner_id).unwrap().unwrap();
        let history_hash1 = v.object_history(object_id1).root_hash().unwrap();
        let history_hash2 = v.object_history(object_id2).root_hash().unwrap();
        let ownership_hash = v.owner_objects(owner_id).root_hash().unwrap();
        let objects_for_owner = v.find_objects_for_owner(owner_id).unwrap();

        assert_eq!(stored_object1.title(), "test object title1");
        assert_eq!(stored_object1.owner_id(), owner_id);
        assert_eq!(stored_object1.points(), &[1.0, 1.0, 3.0, 1.0, 3.0, 4.0]);
        assert_eq!(stored_object1.deleted(), false);
        assert_eq!(stored_object1.history_hash(), &history_hash1);

        assert_eq!(stored_object2.title(), "test object title2");
        assert_eq!(stored_object2.owner_id(), owner_id);
        assert_eq!(stored_object2.points(), &[4.0, 2.0, 5.0, 2.0, 5.0, 4.0]);
        assert_eq!(stored_object2.deleted(), false);
        assert_eq!(stored_object2.history_hash(), &history_hash2);

        assert_eq!(stored_owner.ownership_hash(), &ownership_hash);
        assert_eq!(objects_for_owner.len(), 2);
    }

    #[test]
    fn test_tx_modify_object(){

        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let (p, s) = gen_keypair();
        let tx_create_owner = TxCreateOwner::new(&p, "firstname", "lastname", &s);
        let owner_id = 0_u64;
        let object_id = 0_u64;
        let wrong_object_id = 1_u64;
        let points = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let tx_create_object = TxCreateObject::new(&p, "test object title", &points, owner_id, &s);
        execute_tx::<MemoryDB>(&v, ObjectTx::CreateOwner(tx_create_owner.clone())).unwrap();
        execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object.clone())).unwrap();
        let modified_title = "modified object title";
        let modified_points = vec![5.0, 6.0, 7.0, 8.0];
        let created_at = timestamp();
        let test_tx_modify_object = TxModifyObject::new(&p, object_id, &modified_title, &modified_points, created_at, &s);
        let failed_tx_modify_object = TxModifyObject::new(&p, wrong_object_id, &modified_title, &modified_points, created_at, &s);

        // Act
        let ok_modification_result = execute_tx::<MemoryDB>(&v, ObjectTx::ModifyObject(test_tx_modify_object.clone()));
        let err_modification_result = execute_tx::<MemoryDB>(&v, ObjectTx::ModifyObject(failed_tx_modify_object.clone()));
        let modified_object = v.objects().get(object_id).unwrap().unwrap();
        let history_hash = v.object_history(object_id).root_hash().unwrap();

        // Assert
        assert_eq!(ok_modification_result.is_ok(), true);

        assert_eq!(err_modification_result.is_ok(), true);
        let r = v.results().get(&failed_tx_modify_object.hash()).unwrap().unwrap();
        assert_eq!(r.result(), TxResult::RESULT_MODIFY_WRONG_OBJECT);

        assert_eq!(modified_object.title(), "modified object title");
        assert_eq!(modified_object.owner_id(), owner_id);
        assert_eq!(modified_object.points(), &[5.0, 6.0, 7.0, 8.0]);
        assert_eq!(modified_object.deleted(), false);
        assert_eq!(modified_object.history_hash(), &history_hash);
    }

    #[test]
    fn test_tx_transfer_object(){

        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let (p, s) = gen_keypair();
        let (p2, s2) = gen_keypair();

        let owner_id1 = 0_u64;
        let owner_id2 = 1_u64;
        let tx_create_owner = TxCreateOwner::new(&p, "firstname1", "lastname1", &s);
        let tx_create_owner2 = TxCreateOwner::new(&p2, "firstname2", "lastname2", &s2);
        let object_id = 0_u64;
        let wrong_object_id = 1_u64;
        let points = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let tx_create_object = TxCreateObject::new(&p, "test object title", &points, owner_id1, &s);
        let create_owner_result1 = execute_tx::<MemoryDB>(&v, ObjectTx::CreateOwner(tx_create_owner.clone()));
        let create_owner_result2 = execute_tx::<MemoryDB>(&v, ObjectTx::CreateOwner(tx_create_owner2.clone()));
        let create_object_result = execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object.clone()));
        let created_at = timestamp();
        let success_tx_transfer_object = TxTransferObject::new(&p, object_id, owner_id2, created_at, &s);
        let failed_tx_transfer_object = TxTransferObject::new(&p, wrong_object_id, owner_id2, created_at, &s);

        // Act
        let success_transfer_tx = ObjectTx::TransferObject(success_tx_transfer_object.clone());
        let failed_transfer_tx = ObjectTx::TransferObject(failed_tx_transfer_object.clone());
        let ok_transfer_result = execute_tx::<MemoryDB>(&v, success_transfer_tx.clone());
        let err_transfer_result = execute_tx::<MemoryDB>(&v, failed_transfer_tx.clone());

        let modified_object = v.objects().get(object_id).unwrap().unwrap();
        let modified_owner = v.owners().get(owner_id1).unwrap().unwrap();
        let modified_owner2 = v.owners().get(owner_id2).unwrap().unwrap();
        let history_hash = v.object_history(object_id).root_hash().unwrap();
        let ownership_hash = v.owner_objects(owner_id1).root_hash().unwrap();
        let ownership_hash2 = v.owner_objects(owner_id2).root_hash().unwrap();

        // Assert
        assert_eq!(create_owner_result1.is_ok(), true);
        assert_eq!(create_owner_result2.is_ok(), true);
        assert_eq!(create_object_result.is_ok(), true);

        assert_eq!(ok_transfer_result.is_ok(), true);
        assert_eq!(err_transfer_result.is_ok(), true);
        let r = v.results().get(&failed_transfer_tx.hash()).unwrap().unwrap();
        assert_eq!(r.result(), TxResult::RESULT_TRANSFER_WRONG_OBJECT);

        assert_eq!(modified_object.owner_id(), owner_id2);
        assert_eq!(modified_object.history_hash(), &history_hash);
        assert_eq!(modified_owner.ownership_hash(), &ownership_hash);
        assert_eq!(modified_owner2.ownership_hash(), &ownership_hash2);
    }

    #[test]
    fn test_tx_remove_object(){
        // Arrange
        let b = ObjectsBlockchain { db: MemoryDB::new() };
        let v = b.view();
        let owner_id = 0_u64;
        let (p, s) = gen_keypair();
        let tx_create_owner = TxCreateOwner::new(&p, "firstname", "lastname", &s);
        let object_id = 0_u64;
        let wrong_object_id = 1_u64;
        let points = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let tx_create_object = TxCreateObject::new(&p, "test object title", &points, owner_id, &s);
        execute_tx::<MemoryDB>(&v, ObjectTx::CreateOwner(tx_create_owner.clone())).unwrap();
        execute_tx::<MemoryDB>(&v, ObjectTx::CreateObject(tx_create_object.clone())).unwrap();
        let created_at = timestamp();
        let test_tx_remove_object = TxRemoveObject::new(&p, object_id, created_at, &s);
        let failed_tx_remove_object = TxRemoveObject::new(&p, wrong_object_id, created_at, &s);
        // Act
        let ok_remove_result = execute_tx::<MemoryDB>(&v, ObjectTx::RemoveObject(test_tx_remove_object.clone()));
        let err_remove_result = execute_tx::<MemoryDB>(&v, ObjectTx::RemoveObject(failed_tx_remove_object.clone()));
        let removed_object = v.objects().get(object_id).unwrap().unwrap();
        let history_hash = v.object_history(object_id).root_hash().unwrap();
        // Assert
        assert_eq!(ok_remove_result.is_ok(), true);

        assert_eq!(err_remove_result.is_ok(), true);
        let r = v.results().get(&failed_tx_remove_object.hash()).unwrap().unwrap();
        assert_eq!(r.result(), TxResult::RESULT_REMOVE_WRONG_OBJECT);

        assert_eq!(removed_object.title(), "test object title");
        assert_eq!(removed_object.owner_id(), owner_id);
        assert_eq!(removed_object.points(), &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(removed_object.deleted(), true);
        assert_eq!(removed_object.history_hash(), &history_hash);

    }
}









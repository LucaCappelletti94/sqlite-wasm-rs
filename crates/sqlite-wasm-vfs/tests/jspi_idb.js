// Test-only helpers. Rust guards restore patched browser methods after each case.

export function deleteDatabase(name) {
    return new Promise((resolve, reject) => {
        const request = indexedDB.deleteDatabase(name);
        request.onsuccess = resolve;
        request.onerror = () => reject(request.error);
    });
}

// Names in the `files` store, read directly from IndexedDB.
export function storedFiles(database) {
    return new Promise((resolve, reject) => {
        const open = indexedDB.open(database);
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
            const keys = open.result.transaction("files").objectStore("files").getAllKeys();
            keys.onsuccess = () => {
                open.result.close();
                resolve(keys.result);
            };
            keys.onerror = () => reject(keys.error);
        };
    });
}

// Aborts the transaction of the `after`-th `put` from now, after issuing it.
export function abortAtPut(after) {
    const prototype = IDBObjectStore.prototype;
    const original = prototype.put;
    prototype.put = function (...args) {
        const request = original.apply(this, args);
        if (--after === 0) this.transaction.abort();
        return request;
    };
    return () => { prototype.put = original; };
}

// Makes every flush fail while installed.
export function failPuts() {
    const prototype = IDBObjectStore.prototype;
    const original = prototype.put;
    prototype.put = function () {
        throw new DOMException("injected IndexedDB failure", "UnknownError");
    };
    return () => { prototype.put = original; };
}

// Fails every write to `name` and every deletion of its journal, as a crash
// after the journal reached IndexedDB would leave them.
export function strandJournal(name) {
    const prototype = IDBObjectStore.prototype;
    const { put, delete: remove } = prototype;
    const fail = () => { throw new DOMException("injected IndexedDB failure", "UnknownError"); };
    prototype.put = function (value, key) {
        if (key === name || (Array.isArray(key) && key[0] === name)) fail();
        return put.call(this, value, key);
    };
    prototype.delete = function (key) {
        if (key === `${name}-journal`) fail();
        return remove.call(this, key);
    };
    return () => {
        prototype.put = put;
        prototype.delete = remove;
    };
}

// Records the durability of each read-write transaction until the returned
// function is called, which returns them.
export function recordWrites() {
    const prototype = IDBDatabase.prototype;
    const original = prototype.transaction;
    const writes = [];
    prototype.transaction = function (stores, mode, options) {
        const transaction = original.call(this, stores, mode, options);
        if (mode === "readwrite") writes.push(transaction.durability);
        return transaction;
    };
    return () => {
        prototype.transaction = original;
        return writes;
    };
}

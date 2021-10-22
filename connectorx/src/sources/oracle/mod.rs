mod errors;
mod typesystem;

pub use self::errors::OracleSourceError;
use crate::sql::limit1_query_oracle;
use crate::{
    data_order::DataOrder,
    errors::ConnectorXError,
    sources::{PartitionParser, Produce, Source, SourcePartition},
    sql::{count_query, get_limit, CXQuery},
};
use anyhow::anyhow;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use fehler::{throw, throws};
use log::debug;
use r2d2::{Pool, PooledConnection};
use r2d2_oracle::oracle::ResultSet;
use r2d2_oracle::{
    oracle::{Row, SqlValue},
    OracleConnectionManager,
};
use sqlparser::dialect::Dialect;
use std::{
    rc::Rc,
    sync::mpsc::{channel, Sender},
    thread,
};
pub use typesystem::OracleTypeSystem;
use url::Url;
use urlencoding::decode;

type OracleManager = OracleConnectionManager;
type OracleConn = PooledConnection<OracleManager>;

#[derive(Debug)]
pub struct OracleDialect {}

// implementation copy from AnsiDialect
impl Dialect for OracleDialect {
    fn is_identifier_start(&self, ch: char) -> bool {
        ('a'..='z').contains(&ch) || ('A'..='Z').contains(&ch)
    }

    fn is_identifier_part(&self, ch: char) -> bool {
        ('a'..='z').contains(&ch)
            || ('A'..='Z').contains(&ch)
            || ('0'..='9').contains(&ch)
            || ch == '_'
    }
}

// copy from rust-oracle
struct RowSharedData {
    _column_names: Vec<String>,
}
struct OracleRow {
    _shared: Rc<RowSharedData>,
    pub column_values: Vec<SqlValue>,
}

pub struct OracleSource {
    pool: Pool<OracleManager>,
    queries: Vec<CXQuery<String>>,
    names: Vec<String>,
    schema: Vec<OracleTypeSystem>,
    buf_size: usize,
}

impl OracleSource {
    #[throws(OracleSourceError)]
    pub fn new(conn: &str, nconn: usize) -> Self {
        let conn = Url::parse(conn)?;
        let user = decode(conn.username())?.into_owned();
        let password = decode(conn.password().unwrap_or(""))?.into_owned();
        let host = "//".to_owned() + conn.host_str().unwrap_or("localhost") + conn.path();
        let manager = OracleConnectionManager::new(user.as_str(), password.as_str(), host.as_str());
        let pool = r2d2::Pool::builder()
            .max_size(nconn as u32)
            .build(manager)?;

        Self {
            pool,
            queries: vec![],
            names: vec![],
            schema: vec![],
            buf_size: 32,
        }
    }

    pub fn buf_size(&mut self, buf_size: usize) {
        self.buf_size = buf_size;
    }
}

impl Source for OracleSource
where
    OracleSourcePartition:
        SourcePartition<TypeSystem = OracleTypeSystem, Error = OracleSourceError>,
{
    const DATA_ORDERS: &'static [DataOrder] = &[DataOrder::RowMajor];
    type Partition = OracleSourcePartition;
    type TypeSystem = OracleTypeSystem;
    type Error = OracleSourceError;

    #[throws(OracleSourceError)]
    fn set_data_order(&mut self, data_order: DataOrder) {
        if !matches!(data_order, DataOrder::RowMajor) {
            throw!(ConnectorXError::UnsupportedDataOrder(data_order));
        }
    }

    fn set_queries<Q: ToString>(&mut self, queries: &[CXQuery<Q>]) {
        self.queries = queries.iter().map(|q| q.map(Q::to_string)).collect();
    }

    #[throws(OracleSourceError)]
    fn fetch_metadata(&mut self) {
        assert!(!self.queries.is_empty());

        let conn = self.pool.get()?;
        for (i, query) in self.queries.iter().enumerate() {
            // assuming all the partition queries yield same schema
            match conn.query(limit1_query_oracle(query)?.as_str(), &[]) {
                Ok(rows) => {
                    let (names, types) = rows
                        .column_info()
                        .iter()
                        .map(|col| {
                            (
                                col.name().to_string(),
                                OracleTypeSystem::from(col.oracle_type()),
                            )
                        })
                        .unzip();
                    self.names = names;
                    self.schema = types;
                    return;
                }
                Err(e) if i == self.queries.len() - 1 => {
                    // tried the last query but still get an error
                    debug!("cannot get metadata for '{}': {}", query, e);
                    throw!(e);
                }
                Err(_) => {}
            }
        }
        // tried all queries but all get empty result set
        let iter = conn.query(self.queries[0].as_str(), &[])?;
        let (names, types) = iter
            .column_info()
            .iter()
            .map(|col| (col.name().to_string(), OracleTypeSystem::VarChar(false)))
            .unzip();
        self.names = names;
        self.schema = types;
    }

    fn names(&self) -> Vec<String> {
        self.names.clone()
    }

    fn schema(&self) -> Vec<Self::TypeSystem> {
        self.schema.clone()
    }

    #[throws(OracleSourceError)]
    fn partition(self) -> Vec<Self::Partition> {
        let (tx, rx) = channel::<Option<Vec<()>>>();
        let mut part_num = self.queries.len();
        thread::spawn(move || {
            while part_num > 0 {
                match rx.recv().unwrap() {
                    Some(v) => unsafe {
                        // release SqlValue in a dedicated thread
                        std::mem::transmute::<_, Vec<Vec<SqlValue>>>(v);
                    },
                    None => part_num -= 1, // terminate the thread after receiving # partition of None
                };
            }
            debug!("stop thread for freeing Oracle::SqlValue!");
        });

        let mut ret = vec![];
        for query in self.queries {
            let conn = self.pool.get()?;
            ret.push(OracleSourcePartition::new(
                conn,
                &query,
                &self.schema,
                self.buf_size,
                tx.clone(),
            ));
        }
        ret
    }
}

pub struct OracleSourcePartition {
    conn: OracleConn,
    query: CXQuery<String>,
    schema: Vec<OracleTypeSystem>,
    nrows: usize,
    ncols: usize,
    buf_size: usize,
    sender: Sender<Option<Vec<()>>>,
}

impl OracleSourcePartition {
    pub fn new(
        conn: OracleConn,
        query: &CXQuery<String>,
        schema: &[OracleTypeSystem],
        buf_size: usize,
        sender: Sender<Option<Vec<()>>>,
    ) -> Self {
        Self {
            conn,
            query: query.clone(),
            schema: schema.to_vec(),
            nrows: 0,
            ncols: schema.len(),
            buf_size,
            sender,
        }
    }
}

impl SourcePartition for OracleSourcePartition {
    type TypeSystem = OracleTypeSystem;
    type Parser<'a> = OracleTextSourceParser<'a>;
    type Error = OracleSourceError;

    #[throws(OracleSourceError)]
    fn prepare(&mut self) {
        self.nrows = match get_limit(&self.query, &OracleDialect {})? {
            None => {
                let row = self.conn.query_row_as::<usize>(
                    &count_query(&self.query, &OracleDialect {})?.as_str(),
                    &[],
                )?;
                row
            }
            Some(n) => n,
        };
    }

    #[throws(OracleSourceError)]
    fn parser(&mut self) -> Self::Parser<'_> {
        let query = self.query.clone();
        let iter = self.conn.query(query.as_str(), &[])?;
        OracleTextSourceParser::new(iter, &self.schema, self.buf_size, &self.sender)
    }

    fn nrows(&self) -> usize {
        self.nrows
    }

    fn ncols(&self) -> usize {
        self.ncols
    }
}

pub struct OracleTextSourceParser<'a> {
    iter: ResultSet<'a, Row>,
    buf_size: usize,
    rowbuf: Vec<Row>,
    ncols: usize,
    current_col: usize,
    current_row: usize,
    sender: &'a Sender<Option<Vec<()>>>,
}

impl<'a> OracleTextSourceParser<'a> {
    pub fn new(
        iter: ResultSet<'a, Row>,
        schema: &[OracleTypeSystem],
        buf_size: usize,
        sender: &'a Sender<Option<Vec<()>>>,
    ) -> Self {
        Self {
            iter,
            buf_size,
            rowbuf: Vec::with_capacity(buf_size),
            ncols: schema.len(),
            current_row: 0,
            current_col: 0,
            sender,
        }
    }

    #[throws(OracleSourceError)]
    fn next_loc(&mut self) -> (usize, usize) {
        if self.current_row >= self.rowbuf.len() {
            if !self.rowbuf.is_empty() {
                let b: Vec<Vec<SqlValue>> = self
                    .rowbuf
                    .drain(..)
                    .map(|r| unsafe { std::mem::transmute::<Row, OracleRow>(r) }.column_values)
                    .collect();

                let val: Vec<()> = unsafe { std::mem::transmute(b) };
                self.sender.send(Some(val)).unwrap();
            }

            for _ in 0..self.buf_size {
                if let Some(item) = self.iter.next() {
                    self.rowbuf.push(item?);
                } else {
                    break;
                }
            }

            if self.rowbuf.is_empty() {
                throw!(anyhow!("Oracle EOF"));
            }
            self.current_row = 0;
            self.current_col = 0;
        }
        let ret = (self.current_row, self.current_col);
        self.current_row += (self.current_col + 1) / self.ncols;
        self.current_col = (self.current_col + 1) % self.ncols;
        ret
    }
}

impl<'a> PartitionParser<'a> for OracleTextSourceParser<'a> {
    type TypeSystem = OracleTypeSystem;
    type Error = OracleSourceError;

    fn finalize(&mut self) -> Result<(), Self::Error> {
        self.sender.send(None).unwrap();
        Ok(())
    }
}

macro_rules! impl_produce_text {
    ($($t: ty,)+) => {
        $(
            impl<'r, 'a> Produce<'r, $t> for OracleTextSourceParser<'a> {
                type Error = OracleSourceError;

                #[throws(OracleSourceError)]
                fn produce(&'r mut self) -> $t {
                    let (ridx, cidx) = self.next_loc()?;
                    let res = self.rowbuf[ridx].get(cidx)?;
                    res
                }
            }

            impl<'r, 'a> Produce<'r, Option<$t>> for OracleTextSourceParser<'a> {
                type Error = OracleSourceError;

                #[throws(OracleSourceError)]
                fn produce(&'r mut self) -> Option<$t> {
                    let (ridx, cidx) = self.next_loc()?;
                    let res = self.rowbuf[ridx].get(cidx)?;
                    res
                }
            }
        )+
    };
}

impl_produce_text!(i64, f64, String, NaiveDate, NaiveDateTime, DateTime<Utc>,);

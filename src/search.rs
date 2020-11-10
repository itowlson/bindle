use semver::{Version, VersionReq};
use std::collections::BTreeMap;
use std::ops::RangeInclusive;

/// The search options for performing this query and returning results
pub struct SearchOptions {
    /// The offset from the last search results
    pub offset: u64,
    /// The maximum number of results to return
    pub limit: u8,
    /// Whether to use strict mode (if there are multiple modes supported)
    pub strict: bool,
    /// Whether to return yanked bindles
    pub yanked: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            offset: 0,
            limit: 50,
            strict: false,
            yanked: false,
        }
    }
}

/// Describes the matches that are returned
pub struct Matches {
    /// Whether the search engine used strict mode
    pub strict: bool,
    /// The offset of the first result in the matches
    pub offset: u64,
    /// The maximum number of results this query would have returned
    pub limit: u8,
    /// The total number of matches the search engine located
    ///
    /// In many cases, this will not match the number of results returned on this query
    pub total: u64,
    /// Whether there are more results than the ones returned here
    pub more: bool,
    /// The list of invoices returned as this part of the query
    ///
    /// The length of this Vec will be less than or equal to the limit.
    pub invoices: Vec<crate::Invoice>,
    /// Whether this list includes potentially yanked invoices
    pub yanked: bool,
}

impl Matches {
    fn new(opts: &SearchOptions) -> Self {
        Matches {
            // Assume options are definitive.
            strict: opts.strict,
            offset: opts.offset,
            limit: opts.limit,
            yanked: opts.yanked,

            // Defaults
            invoices: vec![],
            more: false,
            total: 0,
        }
    }
}

/// This trait describes the minimal set of features a Bindle provider must implement
/// to provide query support.
pub trait Search {
    /// A high-level function that can take raw search strings (queries and filters) and options.
    ///
    /// This will parse the terms and filters according to its internal rules, and return
    /// a set of matches.
    ///
    /// An error is returned if either there is something incorrect in the terms/filters,
    /// or if the search engine itself fails to process the query.
    fn query(
        &self,
        term: String,
        filter: String,
        options: SearchOptions,
    ) -> anyhow::Result<Matches>;

    /// Given an invoice, extract information from it that will be useful for searching.
    ///
    /// This high-level feature does not provide any guarantees about how it will
    /// process the invoice. But it may implement Strict and/or Standard modes
    /// described in the protocol specification.
    ///
    /// If the index function is given an invoice it has already indexed, it treats
    /// the call as an update. Otherwise, it adds a new entry to the index.
    ///
    /// As a special note, if an invoice is yanked, the index function will mark it
    /// as such, following the protocol specification's requirements for yanked
    /// invoices.
    fn index(&mut self, document: &crate::Invoice) -> anyhow::Result<()>;
}

/// Implements strict query processing.
pub struct StrictEngine {
    // A BTreeMap will keep the records in a predictable order, which makes the
    // search results predictable. This greatly simplifies the process of doing offsets
    // and limits.
    index: BTreeMap<String, crate::Invoice>,
}

impl Default for StrictEngine {
    fn default() -> Self {
        StrictEngine {
            index: BTreeMap::new(),
        }
    }
}

impl Search for StrictEngine {
    fn query(
        &self,
        term: String,
        filter: String,
        options: SearchOptions,
    ) -> anyhow::Result<Matches> {
        let mut found: Vec<crate::Invoice> = self
            .index
            .iter()
            .filter(|(key, value)| {
                // Term and version have to be exact matches.
                // TODO: Version should have matching turned on.
                *key == &term && version_compare(value.bindle.version.as_str(), &filter)
            })
            .map(|(_, v)| (*v).clone())
            .collect();

        let mut matches = Matches::new(&options);
        matches.strict = true;
        matches.yanked = false;
        matches.total = found.len() as u64;

        if matches.offset >= matches.total {
            // We're past the end of the search results. Return an empty matches object.
            matches.more = false;
            return Ok(matches);
        }

        // Apply offset and limit
        let mut last_index = matches.offset + matches.limit as u64 - 1;
        if last_index >= matches.total {
            last_index = matches.total - 1;
        }

        matches.more = matches.total > last_index + 1;
        let range = RangeInclusive::new(matches.offset as usize, last_index as usize);
        matches.invoices = found.drain(range).collect();

        Ok(matches)
    }

    /// Given an invoice, extract information from it that will be useful for searching.
    ///
    /// This high-level feature does not provide any guarantees about how it will
    /// process the invoice. But it may implement Strict and/or Standard modes
    /// described in the protocol specification.
    ///
    /// If the index function is given an invoice it has already indexed, it treats
    /// the call as an update. Otherwise, it adds a new entry to the index.
    ///
    /// As a special note, if an invoice is yanked, the index function will mark it
    /// as such, following the protocol specification's requirements for yanked
    /// invoices.
    fn index(&mut self, invoice: &crate::Invoice) -> anyhow::Result<()> {
        self.index
            .insert(invoice.bindle.name.clone(), (*invoice).clone());
        Ok(())
    }
}

/// Check whether the given version is within the legal range.
///
/// An empty range matches anything.
///
/// A range that fails to parse matches nothing.
///
/// An empty version matches nothing (unless the requirement is empty)
///
/// A version that fails to parse matches nothing (unless the requirement is empty).
///
/// In all other cases, if the version satisfies the requirement, this returns true.
/// And if it fails to satisfy the requirement, this returns false.
pub fn version_compare(version: &str, requirement: &str) -> bool {
    if requirement.is_empty() {
        return true;
    }

    if let Ok(req) = VersionReq::parse(requirement) {
        println!("Parsed {}", req);
        return match Version::parse(version) {
            Ok(ver) => req.matches(&ver),
            Err(e) => {
                eprintln!("Match failed with an error: {}", e);
                false
            }
        };
    }

    false
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::Invoice;

    #[test]
    fn test_version_comparisons() {
        // Do not need an exhaustive list of matches -- just a sampling to make sure
        // the outer logic is correct.
        let reqs = vec!["= 1.2.3", "1.2.3", "1.2.3", "^1.1", "~1.2", ""];

        reqs.iter().for_each(|r| {
            if !version_compare("1.2.3", r) {
                panic!("Should have passed: {}", r)
            }
        });

        // Again, we do not need to test the SemVer crate -- just make sure some
        // outliers and obvious cases are covered.
        let reqs = vec!["2", "%^&%^&%"];
        reqs.iter()
            .for_each(|r| assert!(!version_compare("1.2.3", r)));

        // Finally, test the outliers having to do with version strings
        let vers = vec!["", "%^&%^&%"];
        vers.iter().for_each(|v| assert!(!version_compare(v, "^1")));
    }

    #[test]
    fn strict_engine_should_index() {
        let inv = invoice_fixture("my/bindle".to_owned(), "1.2.3".to_owned());
        let mut searcher = StrictEngine::default();
        searcher.index(&inv).expect("succesfully indexed my/bindle");
        assert_eq!(1, searcher.index.len());

        // Search for one result
        let matches = searcher
            .query(
                "my/bindle".to_owned(),
                "1.2.3".to_owned(),
                SearchOptions::default(),
            )
            .expect("found some matches");

        assert!(!matches.invoices.is_empty());

        // Search for non-existant bindle
        let matches = searcher
            .query(
                "my/bindle2".to_owned(),
                "1.2.3".to_owned(),
                SearchOptions::default(),
            )
            .expect("found some matches");
        assert!(matches.invoices.is_empty());

        // Search for non-existant version
        let matches = searcher
            .query(
                "my/bindle".to_owned(),
                "1.2.99".to_owned(),
                SearchOptions::default(),
            )
            .expect("found some matches");
        assert!(matches.invoices.is_empty());

        // TODO: Need to test yanked bindles
    }

    fn invoice_fixture(name: String, version: String) -> Invoice {
        let labels = vec![
            crate::Label {
                sha256: "abcdef1234567890987654321".to_owned(),
                media_type: "text/toml".to_owned(),
                name: "foo.toml".to_owned(),
                size: Some(101),
                annotations: None,
            },
            crate::Label {
                sha256: "bbcdef1234567890987654321".to_owned(),
                media_type: "text/toml".to_owned(),
                name: "foo2.toml".to_owned(),
                size: Some(101),
                annotations: None,
            },
            crate::Label {
                sha256: "cbcdef1234567890987654321".to_owned(),
                media_type: "text/toml".to_owned(),
                name: "foo3.toml".to_owned(),
                size: Some(101),
                annotations: None,
            },
        ];

        Invoice {
            bindle_version: crate::BINDLE_VERSION_1.to_owned(),
            yanked: None,
            annotations: None,
            bindle: crate::BindleSpec {
                name,
                version,
                description: Some("bar".to_owned()),
                authors: Some(vec!["m butcher".to_owned()]),
            },
            parcels: Some(
                labels
                    .iter()
                    .map(|l| crate::Parcel {
                        label: l.clone(),
                        conditions: None,
                    })
                    .collect(),
            ),
            group: None,
        }
    }
}
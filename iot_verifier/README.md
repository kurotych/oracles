# IoT Verifier

This crates takes beacon and witness reports as input from an S3 bucket and verifies their integrity against a range of validations and ouputs verified reports to another S3 bucket.  A verified report can either be of type valid or invalid.


## S3 Inputs

- `lora_beacon_ingest_report_v1`: a beacon report produced by the ingestor
- `lora_witness_ingest_report_v1`: a witness report produced by the ingestor
- `entropy_report_v1`: an entropy report produced by the entropy service

## Validations

The verifier will periodically query the incoming S3 bucket and load any new beacon, witness & entropy repots.  These will be used to populate a postres DB.  The DB will then be periodically queried to retrieve any beacon reports with an expired entropy lifespan.  For each such beacon report a list of witnesses will be retrieved and both reports types will then be verified against the following validations:

beacon reports
- `interval check`: is the beaconer permitted to beacon at the current time
- `valid entropy check`:  is the entropy included in the beacon report valid,
- `assertion check`: has the beaconing hotspot been asserted
- `entropy interval check`: was the beacon report received within the associated entropy's lifespan
- `capability check`: is the beaconing hotspot permitted to participate in POC
- `data check`: does the reported broadcast data match that generated dynamically by the verifier

witness reports
- `assertion check`: has the witnessing hotspot been asserted
- `entropy interval check`: was the witness report received within the associated entropy's lifespan
- `frequency check`: does the frequency of the witness report match that of the beaconers
- `region check`: is the witnessing hotspot located in the same region as the beaconer
- `distance check`: is the witnessing hotspot within the permitted distance from the beaconer
- `rssi check`: is the RSSI of the witnessing hotspot valid ( based on free space path loss calc)
- `capability check`: is the beaconing hotspot permitted to participate in POC
- `packet check`: does the reported packet payload match that of the beaconers broadcast


## S3 Outputs

- `valid_poc`: A report consisting of a single valid beacon report and all associated valid witness reports
- `invalid_beacon`: A report consisting of a single invalid beacon report along with its invalid reason
- `invalid_witness`: A report consisting of a single invalid witness report along with its invalid reason

## Env Vars

The verifier requires the following environmental variables:

- `FOLLOWER_URI`: The URI for the follower service to connect to
- `DATABASE_URL`: The URI of the postgres DB instance
- `VERIFIER_STORE`: The path to the tmp local file store for S3 bound files
- `INGESTOR_BUCKET`: The S3 bucket containing the input reports generated by the ingestor
- `VERIFIER_BUCKET`: The S3 bucket to output the verified reports to
- `ENTROPY_BUCKET`: The S3 bucket containing the input entropy reports


## Tunable Consts

The verifier requires the following consts to be sanely set and via which its operation can be tuned

- `BEACON_INTERVAL` (runner) : Interval at which beaconers are permitted to beacon
- `ENTROPY_LIFESPAN` (entropy) : The valid lifespan of a piece of entropy
- `REPORTS_POLL_TIME` (loader) : The cadence at which S3 is queried for new beacon & witness reports
- `ENTROPY_POLL_TIME` ( loader) : The cadence at which S3 is queried for new entropy reports
- `MAX_REPORT_AGE` (loader) : The max age of beacon & witness reports to load from s3. Anything older will be ignored
- `BEACON_MAX_RETRY_ATTEMPTS` (poc_report) : The max number of times the verifier will attempt to verify a beacon
- `WITNESS_MAX_RETRY_ATTEMPTS` (poc_report) : The max number of times the verifier will attempt to verify a witness
- `BEACON_PROCESSING_DELAY` (poc_report) : A period of time added to ENTROPY_LIFESPAN after when any associated beacons using the relevant entropy will become ready for verification
- `POC_DISTANCE_LIMIT` (poc) : The max valid witness distance from the beaconer
- `REPORT_STALE_PERIOD` ( purger) : Any beacon or witness report in the DB & not verified after this period will be deemed stale and purged
- `ENTROPY_STALE_PERIOD( purger) : Any entropy report in the DB after this period will be deemed stale and purged
- `DB_POLL_TIME` ( purger) : The cadence at which the DB is queried for stale reports
- `DB_POLL_TIME` ( runner ) : The cadence at which the DB is queried for 'ready' POCs

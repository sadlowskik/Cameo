# Security

Report vulnerabilities privately. Do not open a public issue for a working
exploit against the daemon, installer, update path, or mesh.

The public wall and mail form: [cameoconstruct.xyz/bounty.html](https://cameoconstruct.xyz/bounty.html).
A name lands there only after the hole is fixed, and only if you asked.

Email **korbin.sadlowski@gmail.com** with:

- the affected version, commit, or ISO tag
- what you did
- what happened
- whether you have a fix

Please give a reasonable window before public disclosure. Cameo is pre-v1;
we still treat remote code execution, auth bypass, and update-trust failures
as urgent.

The update private key and Secure Boot keys are never in this repository.
If you find a private key in a commit or release artifact, write privately
immediately.

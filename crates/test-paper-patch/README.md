# test: paper patching
An *experimental* crate that tests gitpatcher against the [PaperMC](https://papermc.io/) minecraft project.

It is for internal use only, not for consumption by users.

The Paper patch system is the inspiration for this project.
Since Paper includes hundreds of patches, it makes an excellent test case.

This crate is useful for fixing the [excessive churn issue (#1)],
as the PaperMC patch system is already able to filter it out.
